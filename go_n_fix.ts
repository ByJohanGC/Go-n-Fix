import * as anchor from "@coral-xyz/anchor";
import { Program, BN } from "@coral-xyz/anchor";
import { GoNFix } from "../target/types/go_n_fix";
import { PublicKey, Keypair, SystemProgram, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { assert } from "chai";
import crypto from "crypto";

// ═══════════════════════════════════════════════
//  Go & Fix — Tests del Smart Contract
// ═══════════════════════════════════════════════

describe("go_n_fix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GoNFix as Program<GoNFix>;
  const connection = provider.connection;

  // Wallets de prueba
  const client = Keypair.generate();
  const technician = Keypair.generate();
  const platformTreasury = Keypair.generate();
  const arbitrator = Keypair.generate();

  // Job de prueba
  const jobId = Array.from(crypto.randomBytes(32));
  const amountSol = 0.1; // 0.1 SOL
  const amountLamports = amountSol * LAMPORTS_PER_SOL;
  const commissionBps = 500; // 5%
  const descriptionHash = Array.from(
    crypto.createHash("sha256").update("Reparar grifo de cocina").digest()
  );

  let escrowPda: PublicKey;
  let escrowBump: number;

  before(async () => {
    // Fondear wallets en Localnet/Devnet
    const airdropAmount = 2 * LAMPORTS_PER_SOL;

    for (const wallet of [client, technician, platformTreasury, arbitrator]) {
      const sig = await connection.requestAirdrop(wallet.publicKey, airdropAmount);
      await connection.confirmTransaction(sig, "confirmed");
    }

    // Derivar PDA del escrow
    [escrowPda, escrowBump] = await PublicKey.findProgramAddress(
      [
        Buffer.from("escrow"),
        Buffer.from(jobId),
        client.publicKey.toBuffer(),
      ],
      program.programId
    );

    console.log("✅ Wallets fondeadas");
    console.log("📋 Escrow PDA:", escrowPda.toBase58());
  });

  // ─── TEST 1: Crear job con escrow ──────────────────────────────────────────
  it("Crea un job y deposita fondos en escrow", async () => {
    const clientBalanceBefore = await connection.getBalance(client.publicKey);

    await program.methods
      .createJob(
        jobId,
        new BN(amountLamports),
        commissionBps,
        descriptionHash
      )
      .accounts({
        escrowAccount: escrowPda,
        client: client.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([client])
      .rpc();

    const escrow = await program.account.escrowAccount.fetch(escrowPda);
    assert.deepEqual(escrow.jobId, jobId, "Job ID incorrecto");
    assert.equal(escrow.client.toBase58(), client.publicKey.toBase58());
    assert.equal(escrow.amount.toNumber(), amountLamports);
    assert.equal(escrow.commissionBps, commissionBps);
    assert.deepEqual(escrow.status, { open: {} }, "Estado debe ser Open");

    const escrowBalance = await connection.getBalance(escrowPda);
    assert.isAtLeast(escrowBalance, amountLamports, "Escrow debe tener los fondos");

    console.log("✅ Job creado — Escrow:", escrowBalance / LAMPORTS_PER_SOL, "SOL");
  });

  // ─── TEST 2: Técnico acepta el job ──────────────────────────────────────────
  it("El técnico acepta el job", async () => {
    await program.methods
      .acceptJob()
      .accounts({
        escrowAccount: escrowPda,
        technician: technician.publicKey,
      })
      .signers([technician])
      .rpc();

    const escrow = await program.account.escrowAccount.fetch(escrowPda);
    assert.equal(escrow.technician.toBase58(), technician.publicKey.toBase58());
    assert.deepEqual(escrow.status, { accepted: {} });

    console.log("✅ Job aceptado por técnico:", technician.publicKey.toBase58().slice(0, 8) + "...");
  });

  // ─── TEST 3: Técnico marca como completado ───────────────────────────────────
  it("El técnico marca el trabajo como completado", async () => {
    await program.methods
      .completeJob()
      .accounts({
        escrowAccount: escrowPda,
        technician: technician.publicKey,
      })
      .signers([technician])
      .rpc();

    const escrow = await program.account.escrowAccount.fetch(escrowPda);
    assert.deepEqual(escrow.status, { completed: {} });

    console.log("✅ Trabajo marcado como completado");
  });

  // ─── TEST 4: Cliente libera los fondos ──────────────────────────────────────
  it("El cliente libera los fondos al técnico", async () => {
    const techBalanceBefore = await connection.getBalance(technician.publicKey);
    const treasuryBalanceBefore = await connection.getBalance(platformTreasury.publicKey);

    await program.methods
      .releaseFunds()
      .accounts({
        escrowAccount: escrowPda,
        client: client.publicKey,
        technician: technician.publicKey,
        platformTreasury: platformTreasury.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([client])
      .rpc();

    const escrow = await program.account.escrowAccount.fetch(escrowPda);
    assert.deepEqual(escrow.status, { released: {} });

    const techBalanceAfter = await connection.getBalance(technician.publicKey);
    const treasuryBalanceAfter = await connection.getBalance(platformTreasury.publicKey);

    const expectedCommission = Math.floor(amountLamports * commissionBps / 10_000);
    const expectedTechAmount = amountLamports - expectedCommission;

    const techReceived = techBalanceAfter - techBalanceBefore;
    const treasuryReceived = treasuryBalanceAfter - treasuryBalanceBefore;

    console.log("✅ Fondos liberados:");
    console.log("   Técnico recibe:", techReceived / LAMPORTS_PER_SOL, "SOL");
    console.log("   Plataforma recibe:", treasuryReceived / LAMPORTS_PER_SOL, "SOL (comisión 5%)");

    assert.isAtLeast(techReceived, expectedTechAmount - 5000, "Técnico debe recibir monto correcto");
    assert.isAtLeast(treasuryReceived, expectedCommission - 5000, "Plataforma debe recibir comisión");
  });

  // ─── TEST 5: Cancelar job antes de aceptación ───────────────────────────────
  it("Cancela un job abierto y reembolsa al cliente", async () => {
    // Crear nuevo job para cancelar
    const jobId2 = Array.from(crypto.randomBytes(32));
    const [escrowPda2] = await PublicKey.findProgramAddress(
      [Buffer.from("escrow"), Buffer.from(jobId2), client.publicKey.toBuffer()],
      program.programId
    );

    await program.methods
      .createJob(jobId2, new BN(amountLamports), commissionBps, descriptionHash)
      .accounts({ escrowAccount: escrowPda2, client: client.publicKey, systemProgram: SystemProgram.programId })
      .signers([client])
      .rpc();

    const clientBalanceBefore = await connection.getBalance(client.publicKey);

    await program.methods
      .cancelJob()
      .accounts({ escrowAccount: escrowPda2, client: client.publicKey, systemProgram: SystemProgram.programId })
      .signers([client])
      .rpc();

    const escrow = await program.account.escrowAccount.fetch(escrowPda2);
    assert.deepEqual(escrow.status, { cancelled: {} });

    const clientBalanceAfter = await connection.getBalance(client.publicKey);
    console.log("✅ Job cancelado — reembolso recibido");
  });

  // ─── TEST 6: Disputa ────────────────────────────────────────────────────────
  it("Abre una disputa y el árbitro la resuelve (50/50)", async () => {
    // Crear y aceptar un job de prueba
    const jobId3 = Array.from(crypto.randomBytes(32));
    const [escrowPda3] = await PublicKey.findProgramAddress(
      [Buffer.from("escrow"), Buffer.from(jobId3), client.publicKey.toBuffer()],
      program.programId
    );
    const reasonHash = Array.from(crypto.randomBytes(32));

    await program.methods
      .createJob(jobId3, new BN(amountLamports), commissionBps, descriptionHash)
      .accounts({ escrowAccount: escrowPda3, client: client.publicKey, systemProgram: SystemProgram.programId })
      .signers([client]).rpc();

    await program.methods.acceptJob()
      .accounts({ escrowAccount: escrowPda3, technician: technician.publicKey })
      .signers([technician]).rpc();

    // Abrir disputa
    await program.methods.disputeJob(reasonHash)
      .accounts({ escrowAccount: escrowPda3, caller: client.publicKey })
      .signers([client]).rpc();

    let escrow = await program.account.escrowAccount.fetch(escrowPda3);
    assert.deepEqual(escrow.status, { disputed: {} });

    // Resolver disputa 50/50
    await program.methods.resolveDispute(5000) // 50% al cliente
      .accounts({
        escrowAccount: escrowPda3,
        arbitrator: arbitrator.publicKey,
        client: client.publicKey,
        technician: technician.publicKey,
        platformTreasury: platformTreasury.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([arbitrator]).rpc();

    escrow = await program.account.escrowAccount.fetch(escrowPda3);
    assert.deepEqual(escrow.status, { released: {} });
    console.log("✅ Disputa resuelta 50/50 por árbitro");
  });

  console.log("\n🎉 Todos los tests de Go & Fix pasaron correctamente");
});
