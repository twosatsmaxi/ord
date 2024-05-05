use {super::*, crate::outgoing::Outgoing};

pub(crate) struct InscriptionTransfer {
  pub(crate) id: InscriptionId,
}

impl Transfer for InscriptionTransfer {
  fn get_outgoing(&self) -> Outgoing {
    Outgoing::InscriptionId(self.id)
  }

  fn create_unsigned_transaction(
    &self,
    wallet: &Wallet,
    destination: Address,
    postage: Option<Amount>,
    fee_rate: FeeRate,
  ) -> Result<Transaction> {
    let id = self.id;
    let satpoint = wallet
      .inscription_info()
      .get(&id)
      .ok_or_else(|| anyhow!("inscription {id} not found"))?
      .satpoint;
    self.create_unsigned_send_satpoint_transaction(wallet, destination, satpoint, postage, fee_rate)
  }
}
