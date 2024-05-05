use crate::wallet::transfer::amount_tranfer::AmountTransfer;
use crate::wallet::transfer::inscription_transfer::InscriptionTransfer;
use crate::wallet::transfer::rune_transfer::RuneTransfer;
use crate::wallet::transfer::sat_transfer::SatTransfer;
use crate::wallet::transfer::satpoint_transfer::SatPointTransfer;
use crate::wallet::transfer::Transfer;
use {super::*, crate::outgoing::Outgoing};

#[derive(Debug, Parser)]
pub(crate) struct Send {
  #[arg(long, help = "Don't sign or broadcast transaction")]
  pub(crate) dry_run: bool,
  #[arg(long, help = "Use fee rate of <FEE_RATE> sats/vB")]
  fee_rate: FeeRate,
  #[arg(
    long,
    help = "Target <AMOUNT> postage with sent inscriptions. [default: 10000 sat]"
  )]
  pub(crate) postage: Option<Amount>,
  address: Address<NetworkUnchecked>,
  outgoing: Outgoing,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
  pub txid: Txid,
  pub psbt: String,
  pub outgoing: Outgoing,
  pub fee: u64,
}

impl Send {
  pub(crate) fn run(self, wallet: Wallet) -> SubcommandResult {
    let address = self
      .address
      .clone()
      .require_network(wallet.chain().network())?;
    let transfer: Box<dyn Transfer> = match self.outgoing {
      Outgoing::Amount(amount) => Box::new(AmountTransfer { amount }),
      Outgoing::Rune { decimal, rune } => Box::new(RuneTransfer {
        decimal,
        spaced_rune: rune,
      }),
      Outgoing::InscriptionId(id) => Box::new(InscriptionTransfer { id }),
      Outgoing::SatPoint(satpoint) => Box::new(SatPointTransfer { satpoint }),
      Outgoing::Sat(sat) => Box::new(SatTransfer { sat }),
    };
    transfer.send(&wallet, self.dry_run, address, self.postage, self.fee_rate)
  }
}
