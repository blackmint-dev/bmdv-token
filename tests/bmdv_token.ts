import * as anchor from "@coral-xyz/anchor";
import { Program, BN } from "@coral-xyz/anchor";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddress,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";

// Load IDL manually (anchor build --no-idl means we maintain it by hand)
// IDL must have top-level "address" field for Anchor 0.30.1 JS SDK
const IDL = require("../target/idl/bmdv_token.json");

const PROGRAM_ID = new PublicKey(IDL.address);

// Test parameters — small values to conserve devnet SOL
const TOKEN_NAME = "BMDV Test";
const TOKEN_SYMBOL = "BMDVT";
const TOTAL_SUPPLY = new BN(1_000_000); // 1M tokens (raw, no decimals applied here)
const MAX_MINTABLE_SUPPLY = new BN(800_000); // 80% of total supply — public mint cap
const MINT_PRICE = new BN(10_000); // 10,000 lamports per token (0.00001 SOL)
const MAX_PER_WALLET = new BN(500_000);
const MINT_AMOUNT = new BN(100); // Mint 100 tokens in test
const STUDIO_PERCENT = 15;
const CHARITY_PERCENT = 5;
const LP_PERCENT = 80;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

describe("bmdv-token full lifecycle (devnet)", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = new Program(IDL, provider);
  const authority = provider.wallet as anchor.Wallet;

  // Fresh mint keypair for this test run
  const mintKeypair = Keypair.generate();

  // Throwaway wallets for split recipients (devnet only)
  const studioWallet = Keypair.generate();
  const charityWallet = Keypair.generate();
  const lpWallet = Keypair.generate();

  // PDAs — derived once, used across tests
  let mintStatePda: PublicKey;
  let mintStateBump: number;
  let escrowPda: PublicKey;
  let escrowBump: number;
  let userMintRecordPda: PublicKey;
  let userTokenAccount: PublicKey;
  let lpTokenAccount: PublicKey;

  // Mint window: started 60s ago, ends 30s from now
  let mintStart: BN;
  let mintEnd: BN;

  before(async () => {
    // Derive PDAs
    [mintStatePda, mintStateBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("mint-state"), mintKeypair.publicKey.toBuffer()],
      PROGRAM_ID
    );
    [escrowPda, escrowBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), mintKeypair.publicKey.toBuffer()],
      PROGRAM_ID
    );
    [userMintRecordPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("user-record"),
        mintStatePda.toBuffer(),
        authority.publicKey.toBuffer(),
      ],
      PROGRAM_ID
    );

    // Derive ATAs
    userTokenAccount = await getAssociatedTokenAddress(
      mintKeypair.publicKey,
      authority.publicKey
    );
    lpTokenAccount = await getAssociatedTokenAddress(
      mintKeypair.publicKey,
      lpWallet.publicKey
    );

    // Set mint window relative to current on-chain clock
    const slot = await provider.connection.getSlot();
    const blockTime = await provider.connection.getBlockTime(slot);
    const now = blockTime || Math.floor(Date.now() / 1000);
    mintStart = new BN(now - 60); // 60s in the past
    mintEnd = new BN(now + 30); // 30s from now

    console.log(`\n  Authority: ${authority.publicKey.toBase58()}`);
    console.log(`  Mint:      ${mintKeypair.publicKey.toBase58()}`);
    console.log(`  MintState: ${mintStatePda.toBase58()}`);
    console.log(`  Escrow:    ${escrowPda.toBase58()}`);
    console.log(`  Studio:    ${studioWallet.publicKey.toBase58()}`);
    console.log(`  Charity:   ${charityWallet.publicKey.toBase58()}`);
    console.log(`  LP:        ${lpWallet.publicKey.toBase58()}`);
    console.log(`  Mint window: ${mintStart.toString()} to ${mintEnd.toString()} (now: ${now})`);

    // Check balance
    const balance = await provider.connection.getBalance(
      authority.publicKey
    );
    console.log(
      `  Authority balance: ${(balance / LAMPORTS_PER_SOL).toFixed(4)} SOL\n`
    );
    expect(balance).to.be.greaterThan(0.5 * LAMPORTS_PER_SOL);
  });

  it("1. initialize_mint — creates SPL mint, MintState, and Escrow PDAs", async () => {
    const tx = await program.methods
      .initializeMint(
        TOKEN_NAME,
        TOKEN_SYMBOL,
        TOTAL_SUPPLY,
        MAX_MINTABLE_SUPPLY,
        MINT_PRICE,
        mintStart,
        mintEnd,
        MAX_PER_WALLET,
        STUDIO_PERCENT,
        CHARITY_PERCENT,
        LP_PERCENT
      )
      .accounts({
        mintState: mintStatePda,
        mint: mintKeypair.publicKey,
        escrow: escrowPda,
        studioWallet: studioWallet.publicKey,
        charityWallet: charityWallet.publicKey,
        lpWallet: lpWallet.publicKey,
        authority: authority.publicKey,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([mintKeypair])
      .rpc();

    console.log(`    TX: ${tx}`);

    // Verify MintState
    const state = await program.account.mintState.fetch(mintStatePda);
    expect(state.authority.toBase58()).to.equal(
      authority.publicKey.toBase58()
    );
    expect(state.mint.toBase58()).to.equal(
      mintKeypair.publicKey.toBase58()
    );
    expect(state.tokenName).to.equal(TOKEN_NAME);
    expect(state.tokenSymbol).to.equal(TOKEN_SYMBOL);
    expect(state.totalSupply.toNumber()).to.equal(TOTAL_SUPPLY.toNumber());
    expect(state.maxMintableSupply.toNumber()).to.equal(MAX_MINTABLE_SUPPLY.toNumber());
    expect(state.mintedSupply.toNumber()).to.equal(0);
    expect(state.mintPrice.toNumber()).to.equal(MINT_PRICE.toNumber());
    expect(state.studioPercent).to.equal(STUDIO_PERCENT);
    expect(state.charityPercent).to.equal(CHARITY_PERCENT);
    expect(state.lpPercent).to.equal(LP_PERCENT);
    expect(state.isClosed).to.equal(false);
    expect(state.isSupplyFinalized).to.equal(false);
    expect(state.isSplitExecuted).to.equal(false);

    console.log("    MintState verified on-chain");
  });

  it("2. mint_tokens — user pays SOL to escrow, receives tokens", async () => {
    const escrowBefore = await provider.connection.getBalance(escrowPda);

    const tx = await program.methods
      .mintTokens(MINT_AMOUNT)
      .accounts({
        mintState: mintStatePda,
        mint: mintKeypair.publicKey,
        escrow: escrowPda,
        userMintRecord: userMintRecordPda,
        userTokenAccount: userTokenAccount,
        user: authority.publicKey,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    console.log(`    TX: ${tx}`);

    // Verify token balance
    const tokenAcct = await getAccount(
      provider.connection,
      userTokenAccount
    );
    expect(Number(tokenAcct.amount)).to.equal(MINT_AMOUNT.toNumber());

    // Verify escrow received SOL
    const escrowAfter = await provider.connection.getBalance(escrowPda);
    const expectedPayment =
      MINT_AMOUNT.toNumber() * MINT_PRICE.toNumber();
    expect(escrowAfter - escrowBefore).to.equal(expectedPayment);

    // Verify minted_supply updated
    const state = await program.account.mintState.fetch(mintStatePda);
    expect(state.mintedSupply.toNumber()).to.equal(MINT_AMOUNT.toNumber());

    console.log(
      `    Minted ${MINT_AMOUNT} tokens, escrow received ${expectedPayment} lamports`
    );
  });

  it("3. close_mint — authority closes mint after window expires", async () => {
    // Wait for mint window to expire
    const now = Math.floor(Date.now() / 1000);
    const waitTime = mintEnd.toNumber() - now + 5; // +5s buffer
    if (waitTime > 0) {
      console.log(
        `    Waiting ${waitTime}s for mint window to expire...`
      );
      await sleep(waitTime * 1000);
    }

    const tx = await program.methods
      .closeMint()
      .accounts({
        mintState: mintStatePda,
        authority: authority.publicKey,
      })
      .rpc();

    console.log(`    TX: ${tx}`);

    const state = await program.account.mintState.fetch(mintStatePda);
    expect(state.isClosed).to.equal(true);

    console.log("    Mint closed successfully");
  });

  it("4. finalize_supply — mints remaining tokens to LP, revokes mint authority", async () => {
    const stateBefore = await program.account.mintState.fetch(mintStatePda);
    const remaining =
      stateBefore.totalSupply.toNumber() -
      stateBefore.mintedSupply.toNumber();

    const tx = await program.methods
      .finalizeSupply()
      .accounts({
        mintState: mintStatePda,
        mint: mintKeypair.publicKey,
        lpTokenAccount: lpTokenAccount,
        lpWallet: lpWallet.publicKey,
        authority: authority.publicKey,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    console.log(`    TX: ${tx}`);

    // Verify LP received remaining tokens
    const lpAcct = await getAccount(provider.connection, lpTokenAccount);
    expect(Number(lpAcct.amount)).to.equal(remaining);

    // Verify supply finalized
    const state = await program.account.mintState.fetch(mintStatePda);
    expect(state.isSupplyFinalized).to.equal(true);
    expect(state.mintedSupply.toNumber()).to.equal(
      state.totalSupply.toNumber()
    );

    console.log(
      `    ${remaining} tokens minted to LP wallet, mint authority revoked`
    );
  });

  it("5. execute_split — distributes escrow SOL (15/5/80)", async () => {
    // Fund the split recipient wallets with minimum rent so they exist on-chain
    const rentMin = await provider.connection.getMinimumBalanceForRentExemption(0);
    for (const wallet of [studioWallet, charityWallet, lpWallet]) {
      const bal = await provider.connection.getBalance(wallet.publicKey);
      if (bal === 0) {
        const tx = new anchor.web3.Transaction().add(
          SystemProgram.transfer({
            fromPubkey: authority.publicKey,
            toPubkey: wallet.publicKey,
            lamports: rentMin,
          })
        );
        await provider.sendAndConfirm(tx);
      }
    }

    const studioBefore = await provider.connection.getBalance(
      studioWallet.publicKey
    );
    const charityBefore = await provider.connection.getBalance(
      charityWallet.publicKey
    );
    const lpBefore = await provider.connection.getBalance(
      lpWallet.publicKey
    );

    // Get distributable amount (escrow balance minus rent)
    const escrowInfo = await provider.connection.getAccountInfo(escrowPda);
    const escrowRent = await provider.connection.getMinimumBalanceForRentExemption(
      escrowInfo!.data.length
    );
    const distributable = escrowInfo!.lamports - escrowRent;

    const tx = await program.methods
      .executeSplit()
      .accounts({
        mintState: mintStatePda,
        escrow: escrowPda,
        studioWallet: studioWallet.publicKey,
        charityWallet: charityWallet.publicKey,
        lpWallet: lpWallet.publicKey,
        authority: authority.publicKey,
      })
      .rpc();

    console.log(`    TX: ${tx}`);

    const studioAfter = await provider.connection.getBalance(
      studioWallet.publicKey
    );
    const charityAfter = await provider.connection.getBalance(
      charityWallet.publicKey
    );
    const lpAfter = await provider.connection.getBalance(
      lpWallet.publicKey
    );

    const studioGot = studioAfter - studioBefore;
    const charityGot = charityAfter - charityBefore;
    const lpGot = lpAfter - lpBefore;

    // Verify percentages
    const expectedStudio = Math.floor(
      (distributable * STUDIO_PERCENT) / 100
    );
    const expectedCharity = Math.floor(
      (distributable * CHARITY_PERCENT) / 100
    );
    const expectedLp = distributable - expectedStudio - expectedCharity;

    expect(studioGot).to.equal(expectedStudio);
    expect(charityGot).to.equal(expectedCharity);
    expect(lpGot).to.equal(expectedLp);

    // Verify state
    const state = await program.account.mintState.fetch(mintStatePda);
    expect(state.isSplitExecuted).to.equal(true);

    console.log(`    Distributable: ${distributable} lamports`);
    console.log(
      `    Studio:  ${studioGot} (${STUDIO_PERCENT}%)`
    );
    console.log(
      `    Charity: ${charityGot} (${CHARITY_PERCENT}%)`
    );
    console.log(
      `    LP:      ${lpGot} (${LP_PERCENT}% remainder)`
    );
    console.log("    Split executed successfully — full lifecycle complete!");
  });
});
