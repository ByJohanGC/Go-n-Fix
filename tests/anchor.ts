import BN from "bn.js";
import * as web3 from "@solana/web3.js";
import * as anchor from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { GoNFix } from "../target/types/go_n_fix";
import type { GoNFix } from "../target/types/go_n_fix";

describe("go_n_fix", () => {
  // Configure the client to use the local cluster
  anchor.setProvider(anchor.AnchorProvider.env());

  const program = anchor.workspace.GoNFix as anchor.Program<GoNFix>;
  
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.GoNFix as Program<GoNFix>;

  // USAMOS SOLANA WEB3 PARA GENERAR BYTES (Esto no falla en Node)
  const jobId = Array.from(anchor.web3.Keypair.generate().publicKey.toBuffer());
  const descriptionHash = Array.from(anchor.web3.Keypair.generate().publicKey.toBuffer());
  
  // Generamos la wallet del cliente para el test
  const client = anchor.web3.Keypair.generate();

  before(async () => {
    console.log("Preparando fondos para el test...");
    // Transferimos solo 0.08 SOL (suficiente para renta y comisión)
    const tx = new anchor.web3.Transaction().add(
      anchor.web3.SystemProgram.transfer({
        fromPubkey: provider.wallet.publicKey,
        toPubkey: client.publicKey,
        lamports: 0.08 * anchor.web3.LAMPORTS_PER_SOL,
      })
    );
    await provider.sendAndConfirm(tx);
    console.log("✅ Cliente fondeado.");
  });

  it("Crea un job exitosamente", async () => {
    // Calculamos el PDA
    const [escrowPDA] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("escrow"),
        Buffer.from(jobId),
        client.publicKey.toBuffer(),
      ],
      program.programId
    );

    try {
      await program.methods
        .createJob(
          jobId,
          new anchor.BN(0.01 * anchor.web3.LAMPORTS_PER_SOL), // Depósito mínimo de 0.01 SOL
          500, // 5%
          descriptionHash
        )
        .accounts({
          escrowAccount: escrowPDA,
          client: client.publicKey,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .signers([client])
        .rpc();
      
      console.log("✅ ¡LOGRADO! Job creado en:", escrowPDA.toBase58());
    } catch (err) {
      console.error("❌ Falló el RPC:", err);
      throw err;
    }
  });
});