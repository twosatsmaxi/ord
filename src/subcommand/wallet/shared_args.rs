use super::*;

#[derive(Debug, Parser)]
pub(super) struct SharedArgs {
  #[arg(
    long,
    help = "Use <COMMIT_FEE_RATE> sats/vbyte for commit transaction.\nDefaults to <FEE_RATE> if unset."
  )]
  pub(crate) commit_fee_rate: Option<FeeRate>,
  #[arg(long, help = "Compress inscription content with brotli.")]
  pub(crate) compress: bool,
  #[arg(long, help = "Use fee rate of <FEE_RATE> sats/vB.")]
  pub(crate) fee_rate: FeeRate,
  #[arg(long, help = "Don't sign or broadcast transactions.")]
  pub(crate) dry_run: bool,
  #[arg(long, alias = "nobackup", help = "Do not back up recovery key.")]
  pub(crate) no_backup: bool,
  #[clap(long, help = "Don't make a commit transaction; just create a reveal tx that reveals the inscription committed to by output <COMMITMENT>. Requires the same --key as was used to make the commitment. Implies --no-backup. This doesn't work if the --key has ever been backed up to the wallet. When using --commitment, the reveal tx will create a change output unless --reveal-fee is set to '0 sats', in which case the whole commitment will go to postage and fees.")]
  pub(crate) commitment: Option<OutPoint>,
  #[clap(long, help = "Use provided recovery key instead of a random one.")]
  pub(crate) key: Option<String>,
  #[arg(
    long,
    alias = "nolimit",
    help = "Do not check that transactions are equal to or below the MAX_STANDARD_TX_WEIGHT of 400,000 weight units. Transactions over this limit are currently nonstandard and will not be relayed by bitcoind in its default configuration. Do not use this flag unless you understand the implications."
  )]
  pub(crate) no_limit: bool,
}
