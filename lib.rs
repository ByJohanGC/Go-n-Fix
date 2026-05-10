use anchor_lang::prelude::*;
use anchor_lang::system_program;

declare_id!("GoFix1111111111111111111111111111111111111111");

// ═══════════════════════════════════════════════
//  Go & Fix — Smart Contract (Solana / Anchor)
//  Escrow con liberación condicional de fondos
// ═══════════════════════════════════════════════
//
//  Flujo:
//  1. create_job       — Cliente crea el job y deposita fondos en escrow
//  2. accept_job       — Técnico acepta el job (on-chain)
//  3. complete_job     — Técnico marca el trabajo como terminado
//  4. release_funds    — Cliente libera los fondos al técnico (- comisión)
//  5. dispute_job      — Cualquiera abre una disputa (arbitraje futuro)
//  6. cancel_job       — Cliente cancela ANTES de aceptación → reembolso total
//  7. cancel_accepted  — Ambos firman cancelación → reembolso parcial configurable
//
// ═══════════════════════════════════════════════

#[program]
pub mod go_n_fix {
    use super::*;

    // ─── 1. CREAR JOB (depósito en escrow) ───────────────────────────────────
    pub fn create_job(
        ctx: Context<CreateJob>,
        job_id: [u8; 32],       // UUID del job (bytes)
        amount_lamports: u64,   // monto total en lamports
        commission_bps: u16,    // comisión de la plataforma en basis points (ej: 500 = 5%)
        description_hash: [u8; 32], // hash SHA-256 de la descripción (off-chain)
    ) -> Result<()> {
        require!(amount_lamports > 0, GoFixError::InvalidAmount);
        require!(commission_bps <= 3000, GoFixError::CommissionTooHigh); // máx 30%

        let escrow = &mut ctx.accounts.escrow_account;
        escrow.job_id = job_id;
        escrow.client = ctx.accounts.client.key();
        escrow.technician = Pubkey::default(); // vacío hasta accept_job
        escrow.amount = amount_lamports;
        escrow.commission_bps = commission_bps;
        escrow.status = JobStatus::Open;
        escrow.description_hash = description_hash;
        escrow.created_at = Clock::get()?.unix_timestamp;
        escrow.updated_at = Clock::get()?.unix_timestamp;
        escrow.bump = ctx.bumps.escrow_account;

        // Transferir SOL del cliente al escrow PDA
        let cpi_ctx = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.client.to_account_info(),
                to: ctx.accounts.escrow_account.to_account_info(),
            },
        );
        system_program::transfer(cpi_ctx, amount_lamports)?;

        emit!(JobCreated {
            job_id,
            client: ctx.accounts.client.key(),
            amount: amount_lamports,
            commission_bps,
        });

        msg!("Go&Fix: Job creado — {} lamports en escrow", amount_lamports);
        Ok(())
    }

    // ─── 2. ACEPTAR JOB (técnico) ────────────────────────────────────────────
    pub fn accept_job(ctx: Context<AcceptJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;

        require!(escrow.status == JobStatus::Open, GoFixError::InvalidStatus);
        require!(
            escrow.technician == Pubkey::default() || escrow.technician == ctx.accounts.technician.key(),
            GoFixError::Unauthorized
        );

        escrow.technician = ctx.accounts.technician.key();
        escrow.status = JobStatus::Accepted;
        escrow.updated_at = Clock::get()?.unix_timestamp;

        emit!(JobAccepted {
            job_id: escrow.job_id,
            technician: ctx.accounts.technician.key(),
        });

        msg!("Go&Fix: Job aceptado por técnico {}", ctx.accounts.technician.key());
        Ok(())
    }

    // ─── 3. MARCAR TRABAJO COMO COMPLETADO (técnico) ─────────────────────────
    pub fn complete_job(ctx: Context<CompleteJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;

        require!(escrow.status == JobStatus::Accepted, GoFixError::InvalidStatus);
        require!(
            escrow.technician == ctx.accounts.technician.key(),
            GoFixError::Unauthorized
        );

        escrow.status = JobStatus::Completed;
        escrow.updated_at = Clock::get()?.unix_timestamp;

        emit!(JobCompleted {
            job_id: escrow.job_id,
            technician: ctx.accounts.technician.key(),
        });

        msg!("Go&Fix: Trabajo marcado como completado. Esperando liberación de fondos.");
        Ok(())
    }

    // ─── 4. LIBERAR FONDOS (cliente aprueba → paga al técnico) ───────────────
    pub fn release_funds(ctx: Context<ReleaseFunds>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;

        require!(
            escrow.status == JobStatus::Completed || escrow.status == JobStatus::Accepted,
            GoFixError::InvalidStatus
        );
        require!(escrow.client == ctx.accounts.client.key(), GoFixError::Unauthorized);
        require!(escrow.technician == ctx.accounts.technician.key(), GoFixError::InvalidTechnician);

        let total = escrow.amount;
        let commission_bps = escrow.commission_bps as u64;

        // Calcular comisión para la plataforma
        let commission_amount = total
            .checked_mul(commission_bps)
            .ok_or(GoFixError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(GoFixError::MathOverflow)?;

        let tech_amount = total
            .checked_sub(commission_amount)
            .ok_or(GoFixError::MathOverflow)?;

        // Seeds del PDA para firmar las transferencias
        let job_id = escrow.job_id;
        let client_key = escrow.client;
        let bump = escrow.bump;
        let seeds = &[
            b"escrow",
            job_id.as_ref(),
            client_key.as_ref(),
            &[bump],
        ];
        let signer_seeds = &[&seeds[..]];

        // Transferir al técnico
        {
            let from = ctx.accounts.escrow_account.to_account_info();
            let to = ctx.accounts.technician.to_account_info();
            **from.try_borrow_mut_lamports()? -= tech_amount;
            **to.try_borrow_mut_lamports()? += tech_amount;
        }

        // Transferir comisión a la plataforma
        if commission_amount > 0 {
            let from = ctx.accounts.escrow_account.to_account_info();
            let to = ctx.accounts.platform_treasury.to_account_info();
            **from.try_borrow_mut_lamports()? -= commission_amount;
            **to.try_borrow_mut_lamports()? += commission_amount;
        }

        escrow.status = JobStatus::Released;
        escrow.updated_at = Clock::get()?.unix_timestamp;

        emit!(FundsReleased {
            job_id: escrow.job_id,
            technician: ctx.accounts.technician.key(),
            tech_amount,
            commission_amount,
        });

        msg!(
            "Go&Fix: Fondos liberados — Técnico recibe {} lamports, plataforma {} lamports",
            tech_amount,
            commission_amount
        );
        Ok(())
    }

    // ─── 5. ABRIR DISPUTA ────────────────────────────────────────────────────
    pub fn dispute_job(ctx: Context<DisputeJob>, reason_hash: [u8; 32]) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;

        require!(
            escrow.status == JobStatus::Accepted || escrow.status == JobStatus::Completed,
            GoFixError::InvalidStatus
        );

        // Solo el cliente o el técnico pueden abrir disputa
        let caller = ctx.accounts.caller.key();
        require!(
            caller == escrow.client || caller == escrow.technician,
            GoFixError::Unauthorized
        );

        escrow.status = JobStatus::Disputed;
        escrow.dispute_reason_hash = reason_hash;
        escrow.updated_at = Clock::get()?.unix_timestamp;

        emit!(JobDisputed {
            job_id: escrow.job_id,
            raised_by: caller,
        });

        msg!("Go&Fix: Disputa abierta por {}", caller);
        Ok(())
    }

    // ─── 6. CANCELAR JOB (solo antes de aceptación → reembolso total) ────────
    pub fn cancel_job(ctx: Context<CancelJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;

        require!(escrow.status == JobStatus::Open, GoFixError::InvalidStatus);
        require!(escrow.client == ctx.accounts.client.key(), GoFixError::Unauthorized);

        let refund_amount = escrow.amount;

        // Devolver todo al cliente
        {
            let from = ctx.accounts.escrow_account.to_account_info();
            let to = ctx.accounts.client.to_account_info();
            **from.try_borrow_mut_lamports()? -= refund_amount;
            **to.try_borrow_mut_lamports()? += refund_amount;
        }

        escrow.status = JobStatus::Cancelled;
        escrow.updated_at = Clock::get()?.unix_timestamp;

        emit!(JobCancelled {
            job_id: escrow.job_id,
            refund_amount,
        });

        msg!("Go&Fix: Job cancelado — {} lamports reembolsados", refund_amount);
        Ok(())
    }

    // ─── 7. CANCELAR JOB ACEPTADO (ambas partes firman) ─────────────────────
    // El cliente recupera todo MENOS la comisión por trabajo ya iniciado
    pub fn cancel_accepted_job(ctx: Context<CancelAcceptedJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;

        require!(
            escrow.status == JobStatus::Accepted,
            GoFixError::InvalidStatus
        );
        require!(escrow.client == ctx.accounts.client.key(), GoFixError::Unauthorized);
        require!(escrow.technician == ctx.accounts.technician.key(), GoFixError::InvalidTechnician);

        // Penalidad del 5% por cancelación después de aceptación
        let penalty_bps: u64 = 500;
        let penalty = escrow.amount
            .checked_mul(penalty_bps)
            .ok_or(GoFixError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(GoFixError::MathOverflow)?;

        let refund_amount = escrow.amount
            .checked_sub(penalty)
            .ok_or(GoFixError::MathOverflow)?;

        // Reembolso al cliente
        {
            let from = ctx.accounts.escrow_account.to_account_info();
            let to = ctx.accounts.client.to_account_info();
            **from.try_borrow_mut_lamports()? -= refund_amount;
            **to.try_borrow_mut_lamports()? += refund_amount;
        }

        // Penalidad a la plataforma
        if penalty > 0 {
            let from = ctx.accounts.escrow_account.to_account_info();
            let to = ctx.accounts.platform_treasury.to_account_info();
            **from.try_borrow_mut_lamports()? -= penalty;
            **to.try_borrow_mut_lamports()? += penalty;
        }

        escrow.status = JobStatus::Cancelled;
        escrow.updated_at = Clock::get()?.unix_timestamp;

        emit!(JobCancelled {
            job_id: escrow.job_id,
            refund_amount,
        });

        msg!(
            "Go&Fix: Cancelación mutua — reembolso {} lamports, penalidad {} lamports",
            refund_amount,
            penalty
        );
        Ok(())
    }

    // ─── 8. RESOLVER DISPUTA (árbitro de plataforma) ─────────────────────────
    pub fn resolve_dispute(
        ctx: Context<ResolveDispute>,
        client_share_bps: u16,  // % para el cliente en bps (resto va al técnico)
    ) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;

        require!(escrow.status == JobStatus::Disputed, GoFixError::InvalidStatus);
        require!(client_share_bps <= 10_000, GoFixError::InvalidAmount);

        let total = escrow.amount;

        // Comisión de plataforma
        let commission_bps = escrow.commission_bps as u64;
        let commission = total
            .checked_mul(commission_bps)
            .ok_or(GoFixError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(GoFixError::MathOverflow)?;

        let distributable = total.checked_sub(commission).ok_or(GoFixError::MathOverflow)?;

        let client_amount = distributable
            .checked_mul(client_share_bps as u64)
            .ok_or(GoFixError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(GoFixError::MathOverflow)?;

        let tech_amount = distributable
            .checked_sub(client_amount)
            .ok_or(GoFixError::MathOverflow)?;

        // Pagar al técnico
        if tech_amount > 0 {
            let from = ctx.accounts.escrow_account.to_account_info();
            let to = ctx.accounts.technician.to_account_info();
            **from.try_borrow_mut_lamports()? -= tech_amount;
            **to.try_borrow_mut_lamports()? += tech_amount;
        }

        // Reembolsar al cliente
        if client_amount > 0 {
            let from = ctx.accounts.escrow_account.to_account_info();
            let to = ctx.accounts.client.to_account_info();
            **from.try_borrow_mut_lamports()? -= client_amount;
            **to.try_borrow_mut_lamports()? += client_amount;
        }

        // Comisión a la plataforma
        if commission > 0 {
            let from = ctx.accounts.escrow_account.to_account_info();
            let to = ctx.accounts.platform_treasury.to_account_info();
            **from.try_borrow_mut_lamports()? -= commission;
            **to.try_borrow_mut_lamports()? += commission;
        }

        escrow.status = JobStatus::Released;
        escrow.updated_at = Clock::get()?.unix_timestamp;

        emit!(DisputeResolved {
            job_id: escrow.job_id,
            client_amount,
            tech_amount,
            commission,
        });

        msg!(
            "Go&Fix: Disputa resuelta — cliente {} lamports, técnico {} lamports",
            client_amount,
            tech_amount
        );
        Ok(())
    }
}

// ═══════════════════════════════════════════════
//  CONTEXTOS DE INSTRUCCIONES
// ═══════════════════════════════════════════════

#[derive(Accounts)]
#[instruction(job_id: [u8; 32])]
pub struct CreateJob<'info> {
    #[account(
        init,
        payer = client,
        space = EscrowAccount::LEN,
        seeds = [b"escrow", job_id.as_ref(), client.key().as_ref()],
        bump
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    #[account(mut)]
    pub client: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AcceptJob<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump = escrow_account.bump,
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    pub technician: Signer<'info>,
}

#[derive(Accounts)]
pub struct CompleteJob<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump = escrow_account.bump,
        has_one = technician @ GoFixError::Unauthorized,
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    pub technician: Signer<'info>,
}

#[derive(Accounts)]
pub struct ReleaseFunds<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump = escrow_account.bump,
        has_one = client @ GoFixError::Unauthorized,
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    #[account(mut)]
    pub client: Signer<'info>,

    /// CHECK: dirección del técnico verificada contra el escrow
    #[account(mut)]
    pub technician: AccountInfo<'info>,

    /// CHECK: treasury de la plataforma
    #[account(mut)]
    pub platform_treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DisputeJob<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump = escrow_account.bump,
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    pub caller: Signer<'info>,
}

#[derive(Accounts)]
pub struct CancelJob<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump = escrow_account.bump,
        has_one = client @ GoFixError::Unauthorized,
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    #[account(mut)]
    pub client: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CancelAcceptedJob<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump = escrow_account.bump,
        has_one = client @ GoFixError::Unauthorized,
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    #[account(mut)]
    pub client: Signer<'info>,

    /// CHECK: técnico co-firma la cancelación
    #[account(mut)]
    pub technician: Signer<'info>,

    /// CHECK: treasury de la plataforma
    #[account(mut)]
    pub platform_treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ResolveDispute<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump = escrow_account.bump,
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    /// CHECK: árbitro autorizado (en producción usar multisig o DAO)
    #[account(mut)]
    pub arbitrator: Signer<'info>,

    /// CHECK: cliente
    #[account(mut)]
    pub client: AccountInfo<'info>,

    /// CHECK: técnico
    #[account(mut)]
    pub technician: AccountInfo<'info>,

    /// CHECK: treasury
    #[account(mut)]
    pub platform_treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

// ═══════════════════════════════════════════════
//  ESTADO DEL ESCROW
// ═══════════════════════════════════════════════

#[account]
pub struct EscrowAccount {
    pub job_id: [u8; 32],           // UUID del job
    pub client: Pubkey,             // wallet del cliente
    pub technician: Pubkey,         // wallet del técnico (0 si no asignado)
    pub amount: u64,                // lamports en escrow
    pub commission_bps: u16,        // comisión en basis points
    pub status: JobStatus,          // estado actual
    pub description_hash: [u8; 32], // hash SHA-256 de la descripción off-chain
    pub dispute_reason_hash: [u8; 32], // hash de razón de disputa (si aplica)
    pub created_at: i64,            // timestamp Unix
    pub updated_at: i64,            // timestamp Unix
    pub bump: u8,                   // PDA bump
}

impl EscrowAccount {
    // 8 (discriminator) + 32 + 32 + 32 + 8 + 2 + 1 + 32 + 32 + 8 + 8 + 1 = 196
    pub const LEN: usize = 8 + 32 + 32 + 32 + 8 + 2 + 1 + 32 + 32 + 8 + 8 + 1 + 64; // + padding
}

// ═══════════════════════════════════════════════
//  ESTADOS DEL JOB
// ═══════════════════════════════════════════════

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum JobStatus {
    Open,       // creado, esperando técnico
    Accepted,   // técnico aceptó
    Completed,  // técnico marcó como terminado
    Released,   // fondos liberados
    Disputed,   // disputa abierta
    Cancelled,  // cancelado
}

impl Default for JobStatus {
    fn default() -> Self { JobStatus::Open }
}

// ═══════════════════════════════════════════════
//  EVENTOS
// ═══════════════════════════════════════════════

#[event]
pub struct JobCreated {
    pub job_id: [u8; 32],
    pub client: Pubkey,
    pub amount: u64,
    pub commission_bps: u16,
}

#[event]
pub struct JobAccepted {
    pub job_id: [u8; 32],
    pub technician: Pubkey,
}

#[event]
pub struct JobCompleted {
    pub job_id: [u8; 32],
    pub technician: Pubkey,
}

#[event]
pub struct FundsReleased {
    pub job_id: [u8; 32],
    pub technician: Pubkey,
    pub tech_amount: u64,
    pub commission_amount: u64,
}

#[event]
pub struct JobCancelled {
    pub job_id: [u8; 32],
    pub refund_amount: u64,
}

#[event]
pub struct JobDisputed {
    pub job_id: [u8; 32],
    pub raised_by: Pubkey,
}

#[event]
pub struct DisputeResolved {
    pub job_id: [u8; 32],
    pub client_amount: u64,
    pub tech_amount: u64,
    pub commission: u64,
}

// ═══════════════════════════════════════════════
//  ERRORES
// ═══════════════════════════════════════════════

#[error_code]
pub enum GoFixError {
    #[msg("Monto inválido")]
    InvalidAmount,
    #[msg("Comisión no puede superar el 30%")]
    CommissionTooHigh,
    #[msg("Estado del job inválido para esta operación")]
    InvalidStatus,
    #[msg("No autorizado")]
    Unauthorized,
    #[msg("Técnico inválido")]
    InvalidTechnician,
    #[msg("Error matemático — desbordamiento")]
    MathOverflow,
}
