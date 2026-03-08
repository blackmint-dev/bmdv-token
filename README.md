# bmdv-token

Solana SPL token program for BMDV by [blackmint.dev](https://blackmint.dev). 100% public mint, PDA escrow, 15/5/80 split, no team allocation. Entertainment only.

## What This Is

A single Anchor program that handles the full token lifecycle: mint, escrow, and fund distribution. Built for transparency — every SOL that comes in is split on-chain with verifiable receipts.

- **100% of token supply** goes to public mint. Zero pre-mine, zero team allocation.
- **PDA-owned escrow** — only the program can move funds. No admin drain.
- **15/5/80 split** — 15% studio operations, 5% charity, 80% locked LP.
- **Mint authority revoked** after supply finalization. Supply is permanently immutable.
- **LP locked via Streamflow** for 12 months, non-cancelable.

This is an entertainment-only token. No utility, no promises, no roadmap. You can lose everything.

## Program

**Program ID:** `F8VBE7D9aTGysUCWnFQNjgFdM8BmEVcs9UGTJYtLCvJ7`

### Instructions

```
1. initialize_mint     — Create SPL mint, PDA escrow, set split config
2. mint_tokens         — Users pay SOL to PDA escrow, receive tokens
3. close_mint          — Authority closes mint after window ends
4. finalize_supply     — Mint remaining tokens to LP wallet, revoke mint authority
5. execute_split       — Distribute escrow SOL: 15% studio / 5% charity / 80% LP
```

### Lifecycle Enforcement

- `mint_tokens` enforces `max_mintable_supply` (80% of total) — remaining 20% is reserved for LP
- `close_mint` requires mint window to have expired
- `finalize_supply` requires mint to be closed, revokes mint authority permanently
- `execute_split` requires supply to be finalized
- Single authority for the entire lifecycle — no hidden admin functions

### PDAs

| PDA | Seeds | Purpose |
|---|---|---|
| MintState | `["mint-state", mint_pubkey]` | Stores all config, tracks lifecycle state |
| EscrowState | `["escrow", mint_pubkey]` | Holds SOL from minters until split |
| UserMintRecord | `["user-record", mint_state, user]` | Tracks per-wallet mint amount |

## BMDV Token Parameters

| Parameter | Value |
|---|---|
| Total Supply | 10,000,000 |
| Max Mintable (Public) | 8,000,000 (80%) |
| Reserved for LP | 2,000,000 (20%) |
| Mint Price | 0.00003 SOL per token |
| Max Per Wallet | 500,000 (5% of supply) |
| Decimals | 9 |
| Split | 15% studio / 5% charity / 80% LP |

## Build

Requires Anchor 0.30.1, Solana CLI 3.1.10+, Rust stable.

```bash
anchor build --no-idl
```

IDL auto-generation is broken with Anchor 0.30.1 (`proc-macro2` incompatibility). The IDL at `target/idl/bmdv_token.json` is manually maintained in 0.30.1 format.

## Test

Tests run against Solana devnet. Requires a funded devnet wallet.

```bash
ANCHOR_PROVIDER_URL=https://api.devnet.solana.com \
ANCHOR_WALLET=~/.config/solana/id.json \
yarn test
```

**35 tests total:**
- 5 lifecycle tests (full instruction sequence)
- 30 edge case tests (authorization, limits, ordering, error codes)

## Deploy

```bash
# anchor deploy has a known IDL discriminator bug — use solana CLI directly:
solana program deploy target/deploy/bmdv_token.so \
  --program-id target/deploy/bmdv_token-keypair.json \
  --url devnet
```

## Verify

All addresses are published before mint. Verify on Solscan:

- Program: [`F8VBE7D9aTGysUCWnFQNjgFdM8BmEVcs9UGTJYtLCvJ7`](https://solscan.io/account/F8VBE7D9aTGysUCWnFQNjgFdM8BmEVcs9UGTJYtLCvJ7)
- Website: [blackmint.dev](https://blackmint.dev)

## License

MIT
