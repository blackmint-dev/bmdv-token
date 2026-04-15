use anchor_lang::prelude::*;
use anchor_spl::metadata::{
    create_metadata_accounts_v3, CreateMetadataAccountsV3, Metadata,
    mpl_token_metadata::types::DataV2,
};
use anchor_spl::token::{self, Mint, MintTo, Token, TokenAccount};

declare_id!("7ERf7DpQVAEBvxx8LCiN7tat95HqbcqNgVamAsN8pBD5");

pub const MAX_NAME_LEN: usize = 32;
pub const MAX_SYMBOL_LEN: usize = 10;

/// SPL token decimals used for all tokens deployed via this program.
/// Fungible meme tokens MUST have decimals > 0 so Metaplex's
/// create_metadata_accounts_v3 classifies them as TokenStandard::Fungible
/// rather than TokenStandard::FungibleAsset (which wallets render as NFTs).
pub const TOKEN_DECIMALS: u8 = 9;

/// Multiplier to convert whole tokens → SPL base units (10^TOKEN_DECIMALS).
/// MintState tracks amounts in whole tokens everywhere; only the SPL
/// `token::mint_to` CPI boundary needs to use base units.
pub const DECIMAL_MULTIPLIER: u64 = 1_000_000_000;

#[program]
pub mod bmdv_token {
    use super::*;

    /// Initialize a new token mint with split configuration.
    /// Creates the SPL mint, MintState PDA, and EscrowState PDA.
    /// The escrow is a program-owned PDA — only this program can debit it.
    pub fn initialize_mint(
        ctx: Context<InitializeMint>,
        token_name: String,
        token_symbol: String,
        total_supply: u64,
        max_mintable_supply: u64,
        mint_price: u64,
        mint_start: i64,
        mint_end: i64,
        max_per_wallet: u64,
        studio_percent: u8,
        charity_percent: u8,
        lp_percent: u8,
    ) -> Result<()> {
        // Validate mint parameters
        require!(mint_end > mint_start, ErrorCode::InvalidMintWindow);
        require!(total_supply > 0, ErrorCode::InvalidSupply);
        require!(
            max_mintable_supply > 0 && max_mintable_supply <= total_supply,
            ErrorCode::InvalidMaxMintable
        );
        require!(mint_price > 0, ErrorCode::InvalidPrice);
        require!(max_per_wallet > 0, ErrorCode::InvalidLimit);
        require!(token_name.len() <= MAX_NAME_LEN, ErrorCode::NameTooLong);
        require!(token_symbol.len() <= MAX_SYMBOL_LEN, ErrorCode::SymbolTooLong);

        // Validate split percentages
        let total = (studio_percent as u16) + (charity_percent as u16) + (lp_percent as u16);
        require!(total == 100, ErrorCode::InvalidPercentages);

        // Validate split wallets are all distinct
        require!(
            ctx.accounts.studio_wallet.key() != ctx.accounts.charity_wallet.key()
                && ctx.accounts.charity_wallet.key() != ctx.accounts.lp_wallet.key()
                && ctx.accounts.studio_wallet.key() != ctx.accounts.lp_wallet.key(),
            ErrorCode::DuplicateWallets
        );

        // Initialize escrow state
        let escrow = &mut ctx.accounts.escrow;
        escrow.bump = ctx.bumps.escrow;

        // Initialize mint state
        let mint_state = &mut ctx.accounts.mint_state;
        mint_state.authority = ctx.accounts.authority.key();
        mint_state.mint = ctx.accounts.mint.key();
        mint_state.token_name = token_name.clone();
        mint_state.token_symbol = token_symbol.clone();
        mint_state.total_supply = total_supply;
        mint_state.max_mintable_supply = max_mintable_supply;
        mint_state.minted_supply = 0;
        mint_state.mint_price = mint_price;
        mint_state.mint_start = mint_start;
        mint_state.mint_end = mint_end;
        mint_state.max_per_wallet = max_per_wallet;
        mint_state.studio_wallet = ctx.accounts.studio_wallet.key();
        mint_state.charity_wallet = ctx.accounts.charity_wallet.key();
        mint_state.lp_wallet = ctx.accounts.lp_wallet.key();
        mint_state.studio_percent = studio_percent;
        mint_state.charity_percent = charity_percent;
        mint_state.lp_percent = lp_percent;
        mint_state.is_closed = false;
        mint_state.is_supply_finalized = false;
        mint_state.is_split_executed = false;
        mint_state.bump = ctx.bumps.mint_state;
        mint_state.escrow_bump = ctx.bumps.escrow;

        msg!("Token mint initialized: {} ({})", token_name, token_symbol);
        msg!(
            "Supply: {} (max mintable: {}), Price: {} lamports, Window: {} to {}",
            total_supply, max_mintable_supply, mint_price, mint_start, mint_end
        );
        msg!(
            "Split: {}% studio / {}% charity / {}% LP",
            studio_percent, charity_percent, lp_percent
        );

        Ok(())
    }

    /// User mints tokens by paying SOL to the PDA escrow.
    pub fn mint_tokens(ctx: Context<MintTokens>, amount: u64) -> Result<()> {
        require!(amount > 0, ErrorCode::InvalidAmount);

        let mint_state = &ctx.accounts.mint_state;
        let clock = Clock::get()?;

        require!(
            clock.unix_timestamp >= mint_state.mint_start,
            ErrorCode::MintNotStarted
        );
        require!(
            clock.unix_timestamp <= mint_state.mint_end,
            ErrorCode::MintEnded
        );
        require!(!mint_state.is_closed, ErrorCode::MintClosed);

        require!(
            mint_state
                .minted_supply
                .checked_add(amount)
                .ok_or(ErrorCode::Overflow)?
                <= mint_state.max_mintable_supply,
            ErrorCode::SupplyExhausted
        );

        let user_record = &ctx.accounts.user_mint_record;
        require!(
            user_record
                .amount_minted
                .checked_add(amount)
                .ok_or(ErrorCode::Overflow)?
                <= mint_state.max_per_wallet,
            ErrorCode::ExceedsWalletLimit
        );

        // Calculate SOL payment (u128 intermediate to prevent overflow)
        let payment_amount = (amount as u128)
            .checked_mul(mint_state.mint_price as u128)
            .and_then(|v| u64::try_from(v).ok())
            .ok_or(ErrorCode::Overflow)?;

        // Transfer SOL from user to PDA escrow
        // system_instruction::transfer adds lamports to any account regardless of owner
        let transfer_ix = anchor_lang::solana_program::system_instruction::transfer(
            &ctx.accounts.user.key(),
            &ctx.accounts.escrow.to_account_info().key(),
            payment_amount,
        );
        anchor_lang::solana_program::program::invoke(
            &transfer_ix,
            &[
                ctx.accounts.user.to_account_info(),
                ctx.accounts.escrow.to_account_info(),
            ],
        )?;

        // Mint tokens to user's ATA via MintState PDA signer
        let seeds = &[
            b"mint-state",
            mint_state.mint.as_ref(),
            &[mint_state.bump],
        ];
        let signer = &[&seeds[..]];

        // Scale user-supplied whole tokens to SPL base units for the mint_to CPI.
        // MintState tracks amounts in whole tokens (supply, limits, minted counts);
        // SPL token::mint_to expects base units.
        let base_units = (amount as u128)
            .checked_mul(DECIMAL_MULTIPLIER as u128)
            .and_then(|v| u64::try_from(v).ok())
            .ok_or(ErrorCode::Overflow)?;

        token::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.user_token_account.to_account_info(),
                    authority: ctx.accounts.mint_state.to_account_info(),
                },
                signer,
            ),
            base_units,
        )?;

        // Update state with checked arithmetic
        let mint_state_mut = &mut ctx.accounts.mint_state;
        mint_state_mut.minted_supply = mint_state_mut
            .minted_supply
            .checked_add(amount)
            .ok_or(ErrorCode::Overflow)?;

        let user_record = &mut ctx.accounts.user_mint_record;
        user_record.user = ctx.accounts.user.key();
        user_record.amount_minted = user_record
            .amount_minted
            .checked_add(amount)
            .ok_or(ErrorCode::Overflow)?;
        user_record.bump = ctx.bumps.user_mint_record;

        msg!(
            "Minted {} tokens to user. Total: {}/{} (max mintable: {})",
            amount,
            mint_state_mut.minted_supply,
            mint_state_mut.total_supply,
            mint_state_mut.max_mintable_supply
        );

        Ok(())
    }

    /// Close the mint window. Must be called after mint_end has passed.
    pub fn close_mint(ctx: Context<CloseMint>) -> Result<()> {
        let mint_state = &ctx.accounts.mint_state;
        let clock = Clock::get()?;

        require!(
            ctx.accounts.authority.key() == mint_state.authority,
            ErrorCode::Unauthorized
        );
        require!(
            clock.unix_timestamp > mint_state.mint_end,
            ErrorCode::MintStillActive
        );
        require!(!mint_state.is_closed, ErrorCode::AlreadyClosed);

        let mint_state = &mut ctx.accounts.mint_state;
        mint_state.is_closed = true;

        msg!(
            "Mint closed. Minted: {}/{}",
            mint_state.minted_supply,
            mint_state.total_supply
        );

        Ok(())
    }

    /// Finalize token supply: mint any remaining tokens to the LP wallet,
    /// then revoke SPL mint authority (makes supply permanently immutable).
    /// Must always be called after close_mint, even if remaining supply is 0.
    pub fn finalize_supply(ctx: Context<FinalizeSupply>) -> Result<()> {
        let mint_state = &ctx.accounts.mint_state;

        require!(
            ctx.accounts.authority.key() == mint_state.authority,
            ErrorCode::Unauthorized
        );
        require!(mint_state.is_closed, ErrorCode::MintNotClosed);
        require!(
            !mint_state.is_supply_finalized,
            ErrorCode::SupplyAlreadyFinalized
        );

        let remaining = mint_state
            .total_supply
            .checked_sub(mint_state.minted_supply)
            .ok_or(ErrorCode::Overflow)?;

        let seeds = &[
            b"mint-state",
            mint_state.mint.as_ref(),
            &[mint_state.bump],
        ];
        let signer = &[&seeds[..]];

        // Mint remaining tokens to LP wallet (if any)
        if remaining > 0 {
            // Same whole-token → base-unit scaling as mint_tokens.
            let remaining_base_units = (remaining as u128)
                .checked_mul(DECIMAL_MULTIPLIER as u128)
                .and_then(|v| u64::try_from(v).ok())
                .ok_or(ErrorCode::Overflow)?;

            token::mint_to(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    MintTo {
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.lp_token_account.to_account_info(),
                        authority: ctx.accounts.mint_state.to_account_info(),
                    },
                    signer,
                ),
                remaining_base_units,
            )?;

            msg!("Minted {} remaining tokens to LP wallet", remaining);
        }

        // Revoke SPL mint authority — supply is now permanently immutable
        token::set_authority(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::SetAuthority {
                    current_authority: ctx.accounts.mint_state.to_account_info(),
                    account_or_mint: ctx.accounts.mint.to_account_info(),
                },
                signer,
            ),
            token::spl_token::instruction::AuthorityType::MintTokens,
            None,
        )?;

        let mint_state = &mut ctx.accounts.mint_state;
        mint_state.minted_supply = mint_state.total_supply;
        mint_state.is_supply_finalized = true;

        msg!("Mint authority revoked. Supply is permanently immutable.");

        Ok(())
    }

    /// Execute the split: distribute escrow SOL to studio, charity, and LP wallets.
    /// LP wallet receives the remainder (zero dust).
    /// Requires: mint closed and supply finalized.
    pub fn execute_split(ctx: Context<ExecuteSplit>) -> Result<()> {
        let mint_state = &ctx.accounts.mint_state;

        require!(
            ctx.accounts.authority.key() == mint_state.authority,
            ErrorCode::Unauthorized
        );
        require!(mint_state.is_closed, ErrorCode::MintNotClosed);
        require!(
            mint_state.is_supply_finalized,
            ErrorCode::SupplyNotFinalized
        );
        require!(!mint_state.is_split_executed, ErrorCode::AlreadyExecuted);

        // Calculate distributable balance (total minus rent-exempt minimum)
        let escrow_info = ctx.accounts.escrow.to_account_info();
        let rent = Rent::get()?;
        let rent_exempt_min = rent.minimum_balance(escrow_info.data_len());
        let escrow_lamports = escrow_info.lamports();

        require!(escrow_lamports > rent_exempt_min, ErrorCode::EmptyEscrow);

        let distributable = escrow_lamports
            .checked_sub(rent_exempt_min)
            .ok_or(ErrorCode::Overflow)?;

        // Studio and charity get exact percentages; LP gets remainder (zero dust)
        let studio_amount = (distributable as u128)
            .checked_mul(mint_state.studio_percent as u128)
            .and_then(|v| v.checked_div(100))
            .and_then(|v| u64::try_from(v).ok())
            .ok_or(ErrorCode::Overflow)?;

        let charity_amount = (distributable as u128)
            .checked_mul(mint_state.charity_percent as u128)
            .and_then(|v| v.checked_div(100))
            .and_then(|v| u64::try_from(v).ok())
            .ok_or(ErrorCode::Overflow)?;

        let lp_amount = distributable
            .checked_sub(studio_amount)
            .and_then(|v| v.checked_sub(charity_amount))
            .ok_or(ErrorCode::Overflow)?;

        msg!(
            "Splitting {} lamports: studio={} ({}%), charity={} ({}%), LP={} (remainder)",
            distributable,
            studio_amount,
            mint_state.studio_percent,
            charity_amount,
            mint_state.charity_percent,
            lp_amount
        );

        // Raw lamport transfers from program-owned escrow PDA
        **escrow_info.try_borrow_mut_lamports()? -= studio_amount;
        **ctx
            .accounts
            .studio_wallet
            .to_account_info()
            .try_borrow_mut_lamports()? += studio_amount;

        **escrow_info.try_borrow_mut_lamports()? -= charity_amount;
        **ctx
            .accounts
            .charity_wallet
            .to_account_info()
            .try_borrow_mut_lamports()? += charity_amount;

        **escrow_info.try_borrow_mut_lamports()? -= lp_amount;
        **ctx
            .accounts
            .lp_wallet
            .to_account_info()
            .try_borrow_mut_lamports()? += lp_amount;

        let mint_state = &mut ctx.accounts.mint_state;
        mint_state.is_split_executed = true;

        msg!("Split executed successfully");

        Ok(())
    }

    /// Create token metadata via CPI to Metaplex Token Metadata program.
    /// Must be called after initialize_mint and before finalize_supply
    /// (which revokes mint authority, making metadata creation impossible).
    pub fn create_token_metadata(
        ctx: Context<CreateTokenMetadata>,
        name: String,
        symbol: String,
        uri: String,
    ) -> Result<()> {
        let mint_state = &ctx.accounts.mint_state;

        require!(
            ctx.accounts.authority.key() == mint_state.authority,
            ErrorCode::Unauthorized
        );
        require!(
            !mint_state.is_supply_finalized,
            ErrorCode::MetadataAfterFinalize
        );

        let mint_key = mint_state.mint;
        let seeds = &[
            b"mint-state",
            mint_key.as_ref(),
            &[mint_state.bump],
        ];
        let signer = &[&seeds[..]];

        create_metadata_accounts_v3(
            CpiContext::new_with_signer(
                ctx.accounts.token_metadata_program.to_account_info(),
                CreateMetadataAccountsV3 {
                    metadata: ctx.accounts.metadata_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    mint_authority: ctx.accounts.mint_state.to_account_info(),
                    update_authority: ctx.accounts.authority.to_account_info(),
                    payer: ctx.accounts.authority.to_account_info(),
                    system_program: ctx.accounts.system_program.to_account_info(),
                    rent: ctx.accounts.rent.to_account_info(),
                },
                signer,
            ),
            DataV2 {
                name,
                symbol,
                uri,
                seller_fee_basis_points: 0,
                creators: None,
                collection: None,
                uses: None,
            },
            true,  // is_mutable (can revoke update authority later)
            true,  // update_authority_is_signer
            None,  // collection_details
        )?;

        msg!("Token metadata created via Metaplex");

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Account structs
// ---------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeMint<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + MintState::LEN,
        seeds = [b"mint-state", mint.key().as_ref()],
        bump
    )]
    pub mint_state: Account<'info, MintState>,

    #[account(
        init,
        payer = authority,
        mint::decimals = 9,
        mint::authority = mint_state,
    )]
    pub mint: Account<'info, Mint>,

    /// Program-owned escrow PDA to hold SOL during mint.
    #[account(
        init,
        payer = authority,
        space = 8 + EscrowState::LEN,
        seeds = [b"escrow", mint.key().as_ref()],
        bump
    )]
    pub escrow: Account<'info, EscrowState>,

    /// CHECK: Studio wallet to receive studio percentage of SOL
    pub studio_wallet: AccountInfo<'info>,

    /// CHECK: Charity wallet to receive charity percentage of SOL
    pub charity_wallet: AccountInfo<'info>,

    /// CHECK: LP wallet to receive LP percentage of SOL
    pub lp_wallet: AccountInfo<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct MintTokens<'info> {
    #[account(
        mut,
        seeds = [b"mint-state", mint.key().as_ref()],
        bump = mint_state.bump,
    )]
    pub mint_state: Box<Account<'info, MintState>>,

    #[account(
        mut,
        constraint = mint.key() == mint_state.mint @ ErrorCode::InvalidMint
    )]
    pub mint: Box<Account<'info, Mint>>,

    /// Program-owned escrow PDA receiving SOL payments
    #[account(
        mut,
        seeds = [b"escrow", mint.key().as_ref()],
        bump = mint_state.escrow_bump,
    )]
    pub escrow: Box<Account<'info, EscrowState>>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserMintRecord::LEN,
        seeds = [b"user-record", mint_state.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub user_mint_record: Box<Account<'info, UserMintRecord>>,

    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = mint,
        associated_token::authority = user,
    )]
    pub user_token_account: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, anchor_spl::associated_token::AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CloseMint<'info> {
    #[account(
        mut,
        seeds = [b"mint-state", mint_state.mint.as_ref()],
        bump = mint_state.bump,
    )]
    pub mint_state: Account<'info, MintState>,

    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct FinalizeSupply<'info> {
    #[account(
        mut,
        seeds = [b"mint-state", mint.key().as_ref()],
        bump = mint_state.bump,
    )]
    pub mint_state: Account<'info, MintState>,

    #[account(
        mut,
        constraint = mint.key() == mint_state.mint @ ErrorCode::InvalidMint
    )]
    pub mint: Account<'info, Mint>,

    /// LP wallet's token account to receive remaining tokens
    #[account(
        init_if_needed,
        payer = authority,
        associated_token::mint = mint,
        associated_token::authority = lp_wallet,
    )]
    pub lp_token_account: Account<'info, TokenAccount>,

    /// CHECK: LP wallet — validated against stored address
    #[account(
        constraint = lp_wallet.key() == mint_state.lp_wallet @ ErrorCode::InvalidWallet
    )]
    pub lp_wallet: AccountInfo<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, anchor_spl::associated_token::AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct ExecuteSplit<'info> {
    #[account(
        mut,
        seeds = [b"mint-state", mint_state.mint.as_ref()],
        bump = mint_state.bump,
    )]
    pub mint_state: Account<'info, MintState>,

    /// Program-owned escrow PDA holding SOL
    #[account(
        mut,
        seeds = [b"escrow", mint_state.mint.as_ref()],
        bump = mint_state.escrow_bump,
    )]
    pub escrow: Account<'info, EscrowState>,

    /// CHECK: Studio wallet — validated against stored address
    #[account(
        mut,
        constraint = studio_wallet.key() == mint_state.studio_wallet @ ErrorCode::InvalidWallet
    )]
    pub studio_wallet: AccountInfo<'info>,

    /// CHECK: Charity wallet — validated against stored address
    #[account(
        mut,
        constraint = charity_wallet.key() == mint_state.charity_wallet @ ErrorCode::InvalidWallet
    )]
    pub charity_wallet: AccountInfo<'info>,

    /// CHECK: LP wallet — validated against stored address
    #[account(
        mut,
        constraint = lp_wallet.key() == mint_state.lp_wallet @ ErrorCode::InvalidWallet
    )]
    pub lp_wallet: AccountInfo<'info>,

    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct CreateTokenMetadata<'info> {
    #[account(
        seeds = [b"mint-state", mint.key().as_ref()],
        bump = mint_state.bump,
    )]
    pub mint_state: Account<'info, MintState>,

    #[account(
        constraint = mint.key() == mint_state.mint @ ErrorCode::InvalidMint
    )]
    pub mint: Account<'info, Mint>,

    /// CHECK: Created by Metaplex via CPI — PDA derived from
    /// ["metadata", token_metadata_program, mint] by the Metaplex program
    #[account(mut)]
    pub metadata_account: UncheckedAccount<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub token_metadata_program: Program<'info, Metadata>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Escrow account — program-owned PDA that holds SOL during the mint.
/// Only this program can debit it (via execute_split).
#[account]
pub struct EscrowState {
    pub bump: u8, // 1
}

impl EscrowState {
    pub const LEN: usize = 1;
}

/// Core state for a token mint + split lifecycle.
#[account]
pub struct MintState {
    pub authority: Pubkey,       // 32
    pub mint: Pubkey,            // 32
    pub token_name: String,      // 4 + MAX_NAME_LEN
    pub token_symbol: String,    // 4 + MAX_SYMBOL_LEN
    pub total_supply: u64,       // 8
    pub max_mintable_supply: u64, // 8
    pub minted_supply: u64,      // 8
    pub mint_price: u64,         // 8
    pub mint_start: i64,         // 8
    pub mint_end: i64,           // 8
    pub max_per_wallet: u64,     // 8
    pub studio_wallet: Pubkey,   // 32
    pub charity_wallet: Pubkey,  // 32
    pub lp_wallet: Pubkey,       // 32
    pub studio_percent: u8,      // 1
    pub charity_percent: u8,     // 1
    pub lp_percent: u8,          // 1
    pub is_closed: bool,         // 1
    pub is_supply_finalized: bool, // 1
    pub is_split_executed: bool, // 1
    pub bump: u8,                // 1
    pub escrow_bump: u8,         // 1
}

impl MintState {
    pub const LEN: usize = 32   // authority
        + 32                     // mint
        + (4 + MAX_NAME_LEN)     // token_name (4-byte len prefix + max chars)
        + (4 + MAX_SYMBOL_LEN)   // token_symbol
        + 8                      // total_supply
        + 8                      // max_mintable_supply
        + 8                      // minted_supply
        + 8                      // mint_price
        + 8                      // mint_start
        + 8                      // mint_end
        + 8                      // max_per_wallet
        + 32                     // studio_wallet
        + 32                     // charity_wallet
        + 32                     // lp_wallet
        + 1                      // studio_percent
        + 1                      // charity_percent
        + 1                      // lp_percent
        + 1                      // is_closed
        + 1                      // is_supply_finalized
        + 1                      // is_split_executed
        + 1                      // bump
        + 1;                     // escrow_bump
}

/// Per-user minting record (tracks amount minted per wallet).
#[account]
pub struct UserMintRecord {
    pub user: Pubkey,        // 32
    pub amount_minted: u64,  // 8
    pub bump: u8,            // 1
}

impl UserMintRecord {
    pub const LEN: usize = 32 + 8 + 1;
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[error_code]
pub enum ErrorCode {
    #[msg("Mint window end must be after start")]
    InvalidMintWindow,
    #[msg("Total supply must be greater than zero")]
    InvalidSupply,
    #[msg("Mint price must be greater than zero")]
    InvalidPrice,
    #[msg("Max per wallet must be greater than zero")]
    InvalidLimit,
    #[msg("Mint amount must be greater than zero")]
    InvalidAmount,
    #[msg("Token name exceeds 32 characters")]
    NameTooLong,
    #[msg("Token symbol exceeds 10 characters")]
    SymbolTooLong,
    #[msg("Percentages must add up to 100")]
    InvalidPercentages,
    #[msg("Split wallet addresses must all be distinct")]
    DuplicateWallets,
    #[msg("Mint has not started yet")]
    MintNotStarted,
    #[msg("Mint window has ended")]
    MintEnded,
    #[msg("Mint has been closed")]
    MintClosed,
    #[msg("Total supply exhausted")]
    SupplyExhausted,
    #[msg("Exceeds maximum allowed per wallet")]
    ExceedsWalletLimit,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("Mint is still active")]
    MintStillActive,
    #[msg("Mint already closed")]
    AlreadyClosed,
    #[msg("Mint must be closed first")]
    MintNotClosed,
    #[msg("Supply already finalized")]
    SupplyAlreadyFinalized,
    #[msg("Supply must be finalized before split")]
    SupplyNotFinalized,
    #[msg("Split has already been executed")]
    AlreadyExecuted,
    #[msg("Mint account does not match stored address")]
    InvalidMint,
    #[msg("Wallet does not match stored address")]
    InvalidWallet,
    #[msg("Escrow has no distributable funds")]
    EmptyEscrow,
    #[msg("Max mintable supply must be > 0 and <= total supply")]
    InvalidMaxMintable,
    #[msg("Cannot create metadata after supply is finalized (mint authority revoked)")]
    MetadataAfterFinalize,
}
