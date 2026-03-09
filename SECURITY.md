# Security Policy

## Scope

This policy covers the `bmdv-token` Anchor program (Solana smart contract). Third-party integrations (Streamflow, Raydium) and the blackmint.dev website are out of scope.

## Reporting a Vulnerability

If you find a security issue in this contract, please report it privately. **Do not open a public GitHub issue.**

Email: **security@blackmint.dev**

Include:
- Description of the vulnerability
- Steps to reproduce or proof of concept
- Affected instruction(s) or account(s)

## What to Expect

- Acknowledgment within 48 hours
- This is a solo-operated project. There is no bug bounty program.
- Confirmed issues will be disclosed publicly after a fix is deployed.

## Contract Security Design

- PDA-owned escrow — only the program can debit funds
- Mint authority revoked permanently after supply finalization
- No upgrade authority on mainnet (immutable)
- No hidden admin functions or backdoors
- All wallet addresses published before mint
- Full source code, IDL, and tests are public
