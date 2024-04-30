use {super::*, crate::wallet::MaturityError};

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct ResumeOutput {
  pub etchings: Vec<batch::Output>,
}
#[derive(Debug, Parser)]
pub(crate) struct Resume {
  #[arg(long, help = "Don't broadcast transactions.")]
  pub(crate) dry_run: bool,
}

impl Resume {
  pub(crate) fn run(self, wallet: Wallet) -> SubcommandResult {
    let mut etchings = Vec::new();
    loop {
      if SHUTTING_DOWN.load(atomic::Ordering::Relaxed) {
        break;
      }

      for (i, (rune, entry)) in wallet.pending_etchings()?.iter().enumerate() {
        if self.dry_run {
          etchings.push(batch::Output {
            reveal_broadcast: false,
            ..entry.output.clone()
          });
          continue;
        };

        match wallet.is_mature(*rune, &entry.commit) {
          Ok(true) => etchings.push(wallet.send_etching(*rune, entry)?),
          Err(MaturityError::CommitSpent(txid)) => {
            eprintln!("Commitment for rune etching {rune} spent in {txid}");
            etchings.remove(i);
          }
          _ => continue,
        }
      }

      if wallet.pending_etchings()?.is_empty() {
        break;
      }

      if self.dry_run {
        break;
      }

      if !wallet.integration_test() {
        thread::sleep(Duration::from_secs(5));
      }
    }

    Ok(Some(Box::new(ResumeOutput { etchings }) as Box<dyn Output>))
  }
}
