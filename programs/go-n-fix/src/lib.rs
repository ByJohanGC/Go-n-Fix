use anchor_lang::prelude::*;
use anchor_lang::system_program;

// ╔══════════════════════════════════════════════════════════════╗
//  Go & Fix — Smart Contract Completo
//  Funciones: create_job · accept_job · complete_job
//             release_funds · cancel_job · dispute_job
//             resolve_dispute
// ╚══════════════════════════════════════════════════════════════╝

declare_id!("HsLizptS6a98iQuUydcJ1d9cqn2Tz7Rb5cGmwVNVVN2s");

const MAX_COMMISSION_BPS: u16 = 2_000;
const MAX_CLIENT_BPS: u16     = 10_000;

#[error_code]
pub enum GoNFixError {
    #[msg("El trabajo no está en el estado requerido para esta operación")]
    InvalidStatus,
    #[msg("No tienes permiso para realizar esta acción")]
    Unauthorized,
    #[msg("La comisión supera el máximo permitido (20%)")]
    CommissionTooHigh,
    #[msg("El porcentaje al cliente en la disputa supera el 100%")]
    InvalidDisputeSplit,
    #[msg("Fondos insuficientes en el escrow")]
    InsufficientFunds,
}

#[program]
pub mod go_n_fix {
    use super::*;

    pub fn create_job(
        ctx: Context<CreateJob>,
        job_id: [u8; 32],
        amount_lamports: u64,
        commission_bps: u16,
        description_hash: [u8; 32],
    ) -> Result<()> {
        require!(commission_bps <= MAX_COMMISSION_BPS, GoNFixError::CommissionTooHigh);
        require!(amount_lamports > 0, GoNFixError::InsufficientFunds);

        let escrow = &mut ctx.accounts.escrow_account;
        escrow.job_id           = job_id;
        escrow.client           = ctx.accounts.client.key();
        escrow.technician       = Pubkey::default();
        escrow.amount           = amount_lamports;
        escrow.commission_bps   = commission_bps;
        escrow.description_hash = description_hash;
        escrow.status           = JobStatus::Open;
        escrow.bump             = ctx.bumps.escrow_account;

        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.client.to_account_info(),
                    to:   escrow.to_account_info(),
                },
            ),
            amount_lamports,
        )?;

        msg!("Job creado — {} lamports en escrow", amount_lamports);
        Ok(())
    }

    pub fn accept_job(ctx: Context<AcceptJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;
        require!(escrow.status == JobStatus::Open, GoNFixError::InvalidStatus);

        escrow.technician = ctx.accounts.technician.key();
        escrow.status     = JobStatus::Accepted;

        msg!("Job aceptado por {}", ctx.accounts.technician.key());
        Ok(())
    }

    pub fn complete_job(ctx: Context<CompleteJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;
        require!(escrow.status == JobStatus::Accepted, GoNFixError::InvalidStatus);
        require!(escrow.technician == ctx.accounts.technician.key(), GoNFixError::Unauthorized);

        escrow.status = JobStatus::Completed;
        msg!("Trabajo marcado como completado");
        Ok(())
    }

    pub fn release_funds(ctx: Context<ReleaseFunds>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;
        require!(escrow.status == JobStatus::Completed, GoNFixError::InvalidStatus);
        require!(escrow.client == ctx.accounts.client.key(), GoNFixError::Unauthorized);

        let amount      = escrow.amount;
        let commission  = (amount as u128 * escrow.commission_bps as u128 / 10_000) as u64;
        let tech_amount = amount.checked_sub(commission).ok_or(GoNFixError::InsufficientFunds)?;

        let job_id     = escrow.job_id;
        let client_key = escrow.client;
        let bump       = escrow.bump;
        let seeds: &[&[u8]] = &[b"escrow", job_id.as_ref(), client_key.as_ref(), &[bump]];
        let signer = &[seeds];

        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.escrow_account.to_account_info(),
                    to:   ctx.accounts.technician.to_account_info(),
                },
                signer,
            ),
            tech_amount,
        )?;

        if commission > 0 {
            system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.escrow_account.to_account_info(),
                        to:   ctx.accounts.platform_treasury.to_account_info(),
                    },
                    signer,
                ),
                commission,
            )?;
        }

        escrow.status = JobStatus::Released;
        msg!("Fondos liberados — Tecnico: {} | Plataforma: {}", tech_amount, commission);
        Ok(())
    }

    pub fn cancel_job(ctx: Context<CancelJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;
        require!(escrow.status == JobStatus::Open, GoNFixError::InvalidStatus);
        require!(escrow.client == ctx.accounts.client.key(), GoNFixError::Unauthorized);

        let amount     = escrow.amount;
        let job_id     = escrow.job_id;
        let client_key = escrow.client;
        let bump       = escrow.bump;
        let seeds: &[&[u8]] = &[b"escrow", job_id.as_ref(), client_key.as_ref(), &[bump]];
        let signer = &[seeds];

        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.escrow_account.to_account_info(),
                    to:   ctx.accounts.client.to_account_info(),
                },
                signer,
            ),
            amount,
        )?;

        escrow.status = JobStatus::Cancelled;
        msg!("Job cancelado — {} lamports devueltos al cliente", amount);
        Ok(())
    }

    pub fn dispute_job(ctx: Context<DisputeJob>, reason_hash: [u8; 32]) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;
        require!(
            escrow.status == JobStatus::Accepted || escrow.status == JobStatus::Completed,
            GoNFixError::InvalidStatus
        );

        let caller = ctx.accounts.caller.key();
        require!(
            caller == escrow.client || caller == escrow.technician,
            GoNFixError::Unauthorized
        );

        escrow.status = JobStatus::Disputed;
        escrow.description_hash = reason_hash;
        msg!("Disputa abierta por {}", caller);
        Ok(())
    }

    pub fn resolve_dispute(ctx: Context<ResolveDispute>, client_bps: u16) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;
        require!(escrow.status == JobStatus::Disputed, GoNFixError::InvalidStatus);
        require!(client_bps <= MAX_CLIENT_BPS, GoNFixError::InvalidDisputeSplit);

        let amount        = escrow.amount;
        let commission    = (amount as u128 * escrow.commission_bps as u128 / 10_000) as u64;
        let distributable = amount.checked_sub(commission).ok_or(GoNFixError::InsufficientFunds)?;
        let client_amount = (distributable as u128 * client_bps as u128 / 10_000) as u64;
        let tech_amount   = distributable.checked_sub(client_amount).ok_or(GoNFixError::InsufficientFunds)?;

        let job_id     = escrow.job_id;
        let client_key = escrow.client;
        let bump       = escrow.bump;
        let seeds: &[&[u8]] = &[b"escrow", job_id.as_ref(), client_key.as_ref(), &[bump]];
        let signer = &[seeds];

        if client_amount > 0 {
            system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.escrow_account.to_account_info(),
                        to:   ctx.accounts.client.to_account_info(),
                    },
                    signer,
                ),
                client_amount,
            )?;
        }

        if tech_amount > 0 {
            system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.escrow_account.to_account_info(),
                        to:   ctx.accounts.technician.to_account_info(),
                    },
                    signer,
                ),
                tech_amount,
            )?;
        }

        if commission > 0 {
            system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.escrow_account.to_account_info(),
                        to:   ctx.accounts.platform_treasury.to_account_info(),
                    },
                    signer,
                ),
                commission,
            )?;
        }

        escrow.status = JobStatus::Released;
        msg!("Disputa resuelta — Cliente: {} | Tecnico: {} | Plataforma: {}", client_amount, tech_amount, commission);
        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════
//  CONTEXTOS DE CUENTAS
// ══════════════════════════════════════════════════════════════

#[derive(Accounts)]
#[instruction(job_id: [u8; 32])]
pub struct CreateJob<'info> {
    #[account(
        init,
        payer = client,
        space = 8 + EscrowAccount::INIT_SPACE,
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
    #[account(mut)]
    pub escrow_account: Account<'info, EscrowAccount>,
    pub technician: Signer<'info>,
}

#[derive(Accounts)]
pub struct CompleteJob<'info> {
    #[account(mut)]
    pub escrow_account: Account<'info, EscrowAccount>,
    pub technician: Signer<'info>,
}

#[derive(Accounts)]
pub struct ReleaseFunds<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump  = escrow_account.bump
    )]
    pub escrow_account: Account<'info, EscrowAccount>,
    #[account(mut)]
    pub client: Signer<'info>,
    #[account(mut)]
    pub technician: SystemAccount<'info>,
    #[account(mut)]
    pub platform_treasury: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CancelJob<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump  = escrow_account.bump
    )]
    pub escrow_account: Account<'info, EscrowAccount>,
    #[account(mut)]
    pub client: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DisputeJob<'info> {
    #[account(mut)]
    pub escrow_account: Account<'info, EscrowAccount>,
    pub caller: Signer<'info>,
}

#[derive(Accounts)]
pub struct ResolveDispute<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow_account.job_id.as_ref(), escrow_account.client.as_ref()],
        bump  = escrow_account.bump
    )]
    pub escrow_account: Account<'info, EscrowAccount>,
    pub arbitrator: Signer<'info>,
    #[account(mut)]
    pub client: SystemAccount<'info>,
    #[account(mut)]
    pub technician: SystemAccount<'info>,
    #[account(mut)]
    pub platform_treasury: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

// ══════════════════════════════════════════════════════════════
//  CUENTA DE DATOS
// ══════════════════════════════════════════════════════════════

#[account]
#[derive(InitSpace)]
pub struct EscrowAccount {
    pub job_id:           [u8; 32],
    pub client:           Pubkey,
    pub technician:       Pubkey,
    pub amount:           u64,
    pub commission_bps:   u16,
    pub description_hash: [u8; 32],
    pub status:           JobStatus,
    pub bump:             u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, InitSpace)]
pub enum JobStatus {
    Open,
    Accepted,
    Completed,
    Released,
    Cancelled,
    Disputed,
}
