# `create_batch_transactions()` - Comprehensive Documentation

## Overview

`create_batch_transactions()` is the core function responsible for building Bitcoin inscription commit-reveal transactions in the `ord` codebase. It handles UTXO selection, taproot key generation, reveal script construction, and transaction signing.

**Location:** `src/wallet/batch/plan.rs:324-808`

**Purpose:** Generate both commit and reveal transactions for inscribing arbitrary data onto specific satoshis using the Ordinals protocol.

## Function Signature

```rust
pub(crate) fn create_batch_transactions(
  &self,
  wallet_inscriptions: BTreeMap<SatPoint, Vec<InscriptionId>>,
  chain: Chain,
  locked_utxos: BTreeSet<OutPoint>,
  runic_utxos: BTreeSet<OutPoint>,
  mut utxos: BTreeMap<OutPoint, TxOut>,
  commit_change: [Address; 2],
  reveal_change: Address,
) -> Result<Transactions>
```

### Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `wallet_inscriptions` | `BTreeMap<SatPoint, Vec<InscriptionId>>` | Map of existing inscriptions in wallet (to avoid) |
| `chain` | `Chain` | Bitcoin network (Mainnet, Testnet, Signet, Regtest) |
| `locked_utxos` | `BTreeSet<OutPoint>` | UTXOs currently locked (pending transactions) |
| `runic_utxos` | `BTreeSet<OutPoint>` | UTXOs containing runes (avoid spending) |
| `utxos` | `BTreeMap<OutPoint, TxOut>` | All available wallet UTXOs with their outputs |
| `commit_change` | `[Address; 2]` | Two change addresses for commit transaction |
| `reveal_change` | `Address` | Change address for reveal transaction |

### Return Value

Returns `Result<Transactions>` containing:

```rust
pub(crate) struct Transactions {
  pub(crate) rune: Option<RuneInfo>,          // Rune info if etching
  pub(crate) commit_tx: Transaction,          // Commit transaction
  pub(crate) recovery_key_pair: TweakedKeyPair, // Taproot-tweaked recovery key
  pub(crate) reveal_tx: Transaction,          // Reveal transaction (signed)
  pub(crate) total_fees: u64,                 // Combined fees in sats
}
```

## Implementation Flow

### Phase 1: Satpoint Selection (Lines 379-402)

Determines which UTXO to inscribe:

```rust
let satpoint = if self.commitment.is_some() {
  // Using existing commitment: dummy satpoint
  SatPoint::from_str("0000000000000000000000000000000000000000000000000000000000000000:0:0")?
} else if let Some(satpoint) = self.satpoint {
  // User-specified satpoint
  satpoint
} else {
  // AUTO-SELECT: Find first cardinal UTXO
  utxos.iter()
    .find(|(outpoint, txout)| {
      txout.value > 0
      && !inscribed_utxos.contains(outpoint)
      && !locked_utxos.contains(outpoint)
      && !runic_utxos.contains(outpoint)
    })
    .map(|(outpoint, _)| SatPoint { outpoint: *outpoint, offset: 0 })
    .ok_or_else(|| anyhow!("wallet contains no cardinal utxos"))?
}
```

**Cardinal UTXO Criteria:**
- Has value > 0
- Not inscribed (no existing inscriptions)
- Not locked (not in pending transactions)
- Not runic (doesn't contain runes)

### Phase 2: Recovery Key Generation (Lines 433-438)

Creates or loads the keypair for taproot commitment:

```rust
let key_pair = if self.key.is_some() {
  // User-provided key (for reveal-only workflow)
  secp256k1::KeyPair::from_secret_key(
    &secp256k1,
    &PrivateKey::from_wif(&self.key.clone().unwrap())?.inner
  )
} else {
  // RANDOM generation (default)
  UntweakedKeyPair::new(&secp256k1, &mut rand::thread_rng())
}
```

**Security:** Uses cryptographically secure random number generator `rand::thread_rng()`.

### Phase 3: Reveal Script Construction (Lines 443-458)

Builds taproot script containing inscriptions:

```rust
// Extract public key from keypair
let (public_key, _parity) = XOnlyPublicKey::from_keypair(&key_pair);

// Build script: [INSCRIPTIONS] <pubkey> OP_CHECKSIG
let reveal_script = Inscription::append_batch_reveal_script(
  &self.inscriptions,
  ScriptBuf::builder()
    .push_slice(public_key.serialize())
    .push_opcode(opcodes::all::OP_CHECKSIG),
);

// Create taproot tree with reveal script as single leaf
let taproot_spend_info = TaprootBuilder::new()
  .add_leaf(0, reveal_script.clone())?
  .finalize(&secp256k1, public_key)?;

// Generate control block for script-path spending
let control_block = taproot_spend_info
  .control_block(&(reveal_script.clone(), LeafVersion::TapScript))?;

// Derive commitment address
let commit_tx_address = Address::p2tr_tweaked(
  taproot_spend_info.output_key(),
  chain.network()
);
```

**Taproot Structure:**
- **Key-path spend:** Not used (would reveal internal key)
- **Script-path spend:** Used to reveal inscriptions via witness
- **Merkle root:** Commits to reveal script
- **Address:** P2TR address that commits to inscriptions

### Phase 4: Reveal Transaction Construction (Lines 462-502, 583-591)

Builds reveal transaction outputs:

```rust
let mut reveal_inputs = Vec::new();
let mut reveal_outputs = Vec::new();

// Add parent input/output if inscribing child
if let Some(ParentInfo { location, destination, tx_out, .. }) = self.parent_info.clone() {
  reveal_inputs.push(location.outpoint);
  reveal_outputs.push(TxOut {
    script_pubkey: destination.script_pubkey(),
    value: tx_out.value,
  });
}

// Add satpoint inputs for SatPoints mode
if self.mode == Mode::SatPoints {
  for (satpoint, _txout) in self.reveal_satpoints.iter() {
    reveal_inputs.push(satpoint.outpoint);
  }
}

// Placeholder for commitment output (updated later)
reveal_inputs.push(OutPoint::null());

// Add destination outputs for inscriptions
for (i, destination) in self.destinations.iter().enumerate() {
  reveal_outputs.push(TxOut {
    script_pubkey: destination.script_pubkey(),
    value: match self.mode {
      Mode::SeparateOutputs | Mode::SatPoints => self.postages[i].to_sat(),
      Mode::SharedOutput | Mode::SameSat => total_postage,
    },
  });
}
```

**Calculate reveal fee:**

```rust
let (_reveal_tx, reveal_fee) = Self::build_reveal_transaction(
  commit_input,
  &control_block,
  self.reveal_fee_rate,
  reveal_outputs.clone(),
  reveal_inputs.clone(),
  &reveal_script,
);
```

### Phase 5: Rune Etching (Lines 508-579)

If etching a rune, encode runestone:

```rust
if let Some(etching) = self.etching {
  let premine = etching.premine.to_integer(etching.divisibility)?;

  // Add premine output if premine > 0
  if premine > 0 {
    let output = u32::try_from(reveal_outputs.len()).unwrap();
    reveal_outputs.push(TxOut {
      script_pubkey: reveal_change.clone().into(),
      value: TARGET_POSTAGE.to_sat(),
    });
    vout = Some(output);
  }

  // Build runestone
  let inner = Runestone {
    edicts: Vec::new(),
    etching: Some(ordinals::Etching {
      divisibility: (etching.divisibility > 0).then_some(etching.divisibility),
      premine: (premine > 0).then_some(premine),
      rune: Some(etching.rune.rune),
      spacers: (etching.rune.spacers > 0).then_some(etching.rune.spacers),
      symbol: Some(etching.symbol),
      terms: /* minting terms */,
      turbo: etching.turbo,
    }),
    mint: None,
    pointer: (premine > 0).then_some((reveal_outputs.len() - 1).try_into().unwrap()),
  };

  // Encode to OP_RETURN
  let script_pubkey = inner.encipher();

  ensure!(
    self.no_limit || script_pubkey.len() <= 82,
    "runestone greater than maximum OP_RETURN size: {} > 82",
    script_pubkey.len()
  );

  reveal_outputs.push(TxOut { script_pubkey, value: 0 });
}
```

**Runestone Structure:**
- **Edicts:** Empty for etching (used for transfers)
- **Etching:** Rune definition with name, symbol, divisibility, supply
- **Premine:** Premined amount allocated to specific output
- **Terms:** Minting rules (cap, amount, height/offset ranges)
- **Pointer:** Directs runes to output index
- **Encoding:** Varint-encoded in OP_RETURN output

### Phase 6: Commit Transaction Building (Lines 603-626)

Creates commit transaction (or empty if using existing commitment):

```rust
let unsigned_commit_tx = if self.commitment.is_some() {
  // Reveal-only mode: empty commit transaction
  Transaction {
    version: 0,
    lock_time: LockTime::ZERO,
    input: vec![],
    output: vec![],
  }
} else {
  // Build actual commit transaction
  TransactionBuilder::new(
    satpoint,
    wallet_inscriptions,
    utxos.clone(),
    locked_utxos.clone(),
    runic_utxos,
    commit_tx_address.clone(),
    commit_change,
    self.commit_fee_rate,
    if self.commit_only {
      Target::NoChange(reveal_fee + Amount::from_sat(total_postage))
    } else {
      Target::Value(target_value)
    },
  ).build_transaction()?
};

// Output key and commitment for commit-only mode
if self.commit_only {
  eprintln!(
    "use --key {} --commitment {}:0 --reveal-fee-rate 2 to reveal this commitment",
    PrivateKey::new(key_pair.secret_key(), chain.network()).to_wif(),
    unsigned_commit_tx.txid()
  );
}
```

**TransactionBuilder handles:**
- Selecting additional cardinal UTXOs if needed for fees
- Creating change outputs
- Ensuring proper fee rate
- Avoiding inscribed/locked/runic UTXOs

### Phase 7: Reveal Input Setup (Lines 633-658)

Updates reveal transaction inputs with actual commitment:

```rust
let mut vout = 0;
reveal_inputs[commit_input] = if self.commitment.is_some() {
  // Using existing commitment
  if reveal_fee != Amount::from_sat(0) {
    // Update change output with remaining value
    let (change_output_index, _) = reveal_outputs
      .iter()
      .enumerate()
      .find(|(_, output)| output.script_pubkey == reveal_change.script_pubkey())
      .expect("should find change output");

    let new_change_value = self.commitment_output.clone().unwrap().value.to_sat()
      - total_postage
      - reveal_fee.to_sat();

    reveal_outputs[change_output_index].value = new_change_value;
  }
  self.commitment.unwrap()
} else {
  // Find commitment output in commit transaction
  let (internal_vout, _commit_output) = unsigned_commit_tx
    .output
    .iter()
    .enumerate()
    .find(|(_vout, output)| output.script_pubkey == commit_tx_address.script_pubkey())
    .expect("should find sat commit/inscription output");

  vout = internal_vout;
  OutPoint {
    txid: unsigned_commit_tx.txid(),
    vout: internal_vout.try_into().unwrap(),
  }
};
```

### Phase 8: Reveal Transaction Signing (Lines 660-736)

Builds and signs the reveal transaction:

```rust
// Build reveal transaction (2nd time with accurate inputs)
let (mut reveal_tx, _fee) = Self::build_reveal_transaction(
  commit_input,
  &control_block,
  self.reveal_fee_rate,
  reveal_outputs.clone(),
  reveal_inputs,
  &reveal_script,
);

// Collect prevouts for signature hash
let mut prevouts = Vec::new();

if let Some(parent_info) = self.parent_info.clone() {
  prevouts.push(parent_info.tx_out);
}

if self.mode == Mode::SatPoints {
  for (_satpoint, txout) in self.reveal_satpoints.iter() {
    prevouts.push(txout.clone());
  }
}

let prevout = if self.commitment.is_some() {
  TxOut {
    value: self.commitment_output.clone().unwrap().value.to_sat(),
    script_pubkey: self.commitment_output.clone().unwrap().script_pub_key.script()?
  }
} else {
  unsigned_commit_tx.output[vout].clone()
};
prevouts.push(prevout);

// Create sighash for script-path spend
let mut sighash_cache = SighashCache::new(&mut reveal_tx);

let sighash = sighash_cache.taproot_script_spend_signature_hash(
  commit_input,
  &Prevouts::All(&prevouts),
  TapLeafHash::from_script(&reveal_script, LeafVersion::TapScript),
  TapSighashType::Default,
)?;

// Sign with Schnorr signature
let sig = secp256k1.sign_schnorr(
  &secp256k1::Message::from_slice(sighash.as_ref())?,
  &key_pair,
);

// Construct witness
let witness = sighash_cache.witness_mut(commit_input)?;
witness.push(Signature { sig, hash_ty: TapSighashType::Default }.to_vec());
witness.push(reveal_script);
witness.push(&control_block.serialize());
```

**Witness Stack (bottom to top):**
1. Schnorr signature (64 bytes)
2. Reveal script (with inscriptions + pubkey + OP_CHECKSIG)
3. Control block (proves script is in taproot tree)

### Phase 9: Recovery Key Derivation (Lines 740-749)

Derives the recovery key from the tweaked keypair:

```rust
let recovery_key_pair = key_pair.tap_tweak(&secp256k1, taproot_spend_info.merkle_root());

// Verify recovery key matches commitment address
let (x_only_pub_key, _parity) = recovery_key_pair.to_inner().x_only_public_key();
assert_eq!(
  Address::p2tr_tweaked(
    TweakedPublicKey::dangerous_assume_tweaked(x_only_pub_key),
    chain.network(),
  ),
  commit_tx_address
);
```

**Key Relationship:**
```
Untweaked Private Key (random or user-provided)
  ↓ tap_tweak(merkle_root)
Tweaked Private Key (recovery key)
  ↓ to_public()
Tweaked Public Key
  ↓ p2tr_tweaked()
Commit Address (P2TR)
```

### Phase 10: Validation & Return (Lines 751-807)

Final validation and return:

```rust
// Check transaction weight
let reveal_weight = reveal_tx.weight();
if !self.no_limit && reveal_weight > bitcoin::Weight::from_wu(MAX_STANDARD_TX_WEIGHT.into()) {
  bail!(
    "reveal transaction weight greater than {MAX_STANDARD_TX_WEIGHT} (MAX_STANDARD_TX_WEIGHT): {reveal_weight}"
  );
}

// Add commitment output to UTXO set for fee calculation
utxos.insert(
  reveal_tx.input[commit_input].previous_output,
  if self.commitment.is_some() {
    TxOut {
      value: self.commitment_output.clone().unwrap().value.to_sat(),
      script_pubkey: self.commitment_output.clone().unwrap().script_pub_key.script()?,
    }
  } else {
    unsigned_commit_tx.output[reveal_tx.input[commit_input].previous_output.vout as usize].clone()
  },
);

// Calculate total fees
let total_fees = if self.commitment.is_some() {
  0  // No commit fee when using existing commitment
} else {
  Self::calculate_fee(&unsigned_commit_tx, &utxos)
    + if !self.commit_only { Self::calculate_fee(&reveal_tx, &utxos) } else { 0 }
};

// Verify runestone encoding
match (Runestone::decipher(&reveal_tx), runestone) {
  (Some(actual), Some(expected)) => assert_eq!(
    actual,
    Artifact::Runestone(expected),
    "commit transaction runestone did not match expected runestone"
  ),
  (Some(_), None) => panic!("commit transaction contained runestone, but none was expected"),
  (None, Some(_)) => panic!("commit transaction did not contain runestone, but one was expected"),
  (None, None) => {}
}

// Build rune info
let rune = rune.map(|(destination, rune, vout)| RuneInfo {
  destination: destination.map(|destination| uncheck(&destination)),
  location: vout.map(|vout| OutPoint { txid: reveal_tx.txid(), vout }),
  rune,
});

// Return transactions
Ok(Transactions {
  commit_tx: unsigned_commit_tx,
  recovery_key_pair,
  reveal_tx,
  rune,
  total_fees,
})
```

## Helper Functions

### `build_reveal_transaction()` (Lines 841-888)

Constructs reveal transaction with witness size estimation:

```rust
fn build_reveal_transaction(
  commit_input_index: usize,
  control_block: &ControlBlock,
  fee_rate: FeeRate,
  output: Vec<TxOut>,
  input: Vec<OutPoint>,
  script: &Script,
) -> (Transaction, Amount) {
  // Build transaction
  let reveal_tx = Transaction {
    input: input
      .into_iter()
      .map(|previous_output| TxIn {
        previous_output,
        script_sig: script::Builder::new().into_script(),
        witness: Witness::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
      })
      .collect(),
    output,
    lock_time: LockTime::ZERO,
    version: 2,
  };

  // Calculate fee with dummy witnesses
  let fee = {
    let mut reveal_tx = reveal_tx.clone();

    for (current_index, txin) in reveal_tx.input.iter_mut().enumerate() {
      if current_index == commit_input_index {
        // Add dummy inscription witness for commit input
        txin.witness.push(Signature::from_slice(&[0; SCHNORR_SIGNATURE_SIZE])?.to_vec());
        txin.witness.push(script);
        txin.witness.push(&control_block.serialize());
      } else {
        // Add dummy witness for other inputs
        txin.witness = Witness::from_slice(&[&[0; SCHNORR_SIGNATURE_SIZE]]);
      }
    }

    fee_rate.fee(reveal_tx.vsize())
  };

  (reveal_tx, fee)
}
```

**Purpose:** Creates reveal transaction template and estimates fee based on vsize with dummy witnesses.

### `calculate_fee()` (Lines 890-897)

Simple fee calculation:

```rust
fn calculate_fee(tx: &Transaction, utxos: &BTreeMap<OutPoint, TxOut>) -> u64 {
  tx.input
    .iter()
    .map(|txin| utxos.get(&txin.previous_output).unwrap().value)
    .sum::<u64>()
    .checked_sub(tx.output.iter().map(|txout| txout.value).sum::<u64>())
    .unwrap()
}
```

**Formula:** `fee = sum(inputs) - sum(outputs)`

### `get_recovery_key()` (Lines 52-66)

Formats recovery key as Bitcoin descriptor:

```rust
fn get_recovery_key(
  client: &Client,
  recovery_key_pair: TweakedKeyPair,
  network: Network,
) -> Result<String> {
  let recovery_private_key = PrivateKey::new(
    recovery_key_pair.to_inner().secret_key(),
    network
  ).to_wif();

  Ok(format!(
    "rawtr({})#{}",
    recovery_private_key,
    client.get_descriptor_info(&format!("rawtr({})", recovery_private_key))?.checksum
  ))
}
```

**Format:** `rawtr(WIF_PRIVATE_KEY)#CHECKSUM`

**Example:** `rawtr(cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy)#abcd1234`

### `backup_recovery_key()` (Lines 810-839)

Imports recovery key to wallet:

```rust
fn backup_recovery_key(wallet: &Wallet, recovery_key_pair: TweakedKeyPair) -> Result {
  let recovery_private_key = PrivateKey::new(
    recovery_key_pair.to_inner().secret_key(),
    wallet.chain().network(),
  );

  let info = wallet
    .bitcoin_client()
    .get_descriptor_info(&format!("rawtr({})", recovery_private_key.to_wif()))?;

  let response = wallet
    .bitcoin_client()
    .import_descriptors(vec![ImportDescriptors {
      descriptor: format!("rawtr({})#{}", recovery_private_key.to_wif(), info.checksum),
      timestamp: Timestamp::Now,
      active: Some(false),
      range: None,
      next_index: None,
      internal: Some(false),
      label: Some("commit tx recovery key".to_string()),
    }])?;

  for result in response {
    if !result.success {
      return Err(anyhow!("commit tx recovery key import failed"));
    }
  }

  Ok(())
}
```

**Backup Logic (plan.rs:163-166):**

```rust
if !self.no_backup && self.key.is_none() && !self.commit_only {
  Self::backup_recovery_key(wallet, recovery_key_pair)?;
}
```

**Backed up IF:**
- `--no-backup` flag is NOT set
- Key was randomly generated (not user-provided)
- Not in commit-only mode

## Usage Examples

### Example 1: Basic Single Inscription

```rust
let plan = batch::Plan {
  satpoint: Some(satpoint(1, 0)),
  parent_info: None,
  inscriptions: vec![inscription("text/plain", "Hello, Ordinals!")],
  destinations: vec![reveal_address],
  commit_fee_rate: FeeRate::try_from(5.0).unwrap(),
  reveal_fee_rate: FeeRate::try_from(5.0).unwrap(),
  no_limit: false,
  reinscribe: false,
  postages: vec![TARGET_POSTAGE],
  mode: batch::Mode::SharedOutput,
  commitment: None,
  commitment_output: None,
  key: None,
  ..Default::default()
};

let batch::Transactions {
  commit_tx,
  reveal_tx,
  recovery_key_pair,
  total_fees,
  ..
} = plan.create_batch_transactions(
  BTreeMap::new(),
  Chain::Mainnet,
  BTreeSet::new(),
  BTreeSet::new(),
  utxos.into_iter().collect(),
  [change_address_1, change_address_2],
  reveal_change_address,
)?;
```

### Example 2: Batch Inscriptions (Separate Outputs)

```rust
let plan = batch::Plan {
  satpoint: None,  // Auto-select cardinal UTXO
  parent_info: None,
  inscriptions: vec![
    inscription("text/plain", "Inscription 1"),
    inscription("text/plain", "Inscription 2"),
    inscription("text/plain", "Inscription 3"),
  ],
  destinations: vec![address_1, address_2, address_3],
  commit_fee_rate: FeeRate::try_from(10.0).unwrap(),
  reveal_fee_rate: FeeRate::try_from(10.0).unwrap(),
  postages: vec![
    Amount::from_sat(10_000),
    Amount::from_sat(10_000),
    Amount::from_sat(10_000),
  ],
  mode: batch::Mode::SeparateOutputs,
  ..Default::default()
};

let txs = plan.create_batch_transactions(/* ... */)?;
```

### Example 3: Commit-Only Workflow

```rust
let plan = batch::Plan {
  inscriptions: vec![inscription("image/png", image_data)],
  destinations: vec![destination_address],
  commit_fee_rate: FeeRate::try_from(20.0).unwrap(),
  reveal_fee_rate: FeeRate::try_from(1.0).unwrap(),
  commit_only: true,  // Only create commit transaction
  no_backup: false,   // Will NOT backup (implied by commit_only)
  ..Default::default()
};

let txs = plan.create_batch_transactions(/* ... */)?;

// Output: "use --key cVt4o7... --commitment abc123:0 --reveal-fee-rate 1"
```

### Example 4: Reveal-Only Workflow

```rust
let plan = batch::Plan {
  inscriptions: vec![inscription("image/png", image_data)],
  destinations: vec![destination_address],
  reveal_fee_rate: FeeRate::try_from(5.0).unwrap(),
  commitment: Some(OutPoint::from_str("abc123...:0")?),
  commitment_output: Some(/* fetched from RPC */),
  key: Some("cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy".to_string()),
  ..Default::default()
};

let txs = plan.create_batch_transactions(/* ... */)?;
// commit_tx will be empty (version 0, no inputs/outputs)
```

### Example 5: Parent-Child Inscription

```rust
let parent_info = Some(ParentInfo {
  id: parent_inscription_id,
  location: parent_satpoint,
  destination: parent_destination,
  tx_out: parent_txout,
});

let plan = batch::Plan {
  inscriptions: vec![inscription_with_parent],
  parent_info,  // Parent will be spent and recreated
  destinations: vec![child_destination],
  ..Default::default()
};

let txs = plan.create_batch_transactions(/* ... */)?;
```

### Example 6: Rune Etching

```rust
let plan = batch::Plan {
  inscriptions: vec![inscription("text/plain", "Rune Launch")],
  destinations: vec![destination],
  etching: Some(batch::Etching {
    rune: SpacedRune { rune: Rune::from_str("MYTOKEN")?, spacers: 0 },
    divisibility: 8,
    premine: DecimalSat::from_str("1000.0")?,
    symbol: '₿',
    terms: Some(batch::Terms {
      cap: 1_000_000,
      amount: DecimalSat::from_str("100.0")?,
      offset: Some(Range { start: Some(0), end: Some(10_000) }),
      height: None,
    }),
    turbo: false,
  }),
  ..Default::default()
};

let txs = plan.create_batch_transactions(/* ... */)?;
```

## Tests

### Location

**Main tests:** `src/wallet/batch.rs:70-1200+`
**Test utilities:** `src/test.rs`

### Key Test Functions

#### `inscription(content_type, body)` - Create Test Inscription

```rust
pub(crate) fn inscription(content_type: &str, body: impl AsRef<[u8]>) -> Inscription {
  Inscription {
    content_type: Some(content_type.into()),
    body: Some(body.as_ref().into()),
    ..default()
  }
}
```

**Usage:**
```rust
let inscription = inscription("text/plain", "Hello World");
let inscription = inscription("image/png", include_bytes!("test.png"));
```

#### Test Helper Functions

```rust
// Create mock UTXO
pub(crate) fn outpoint(n: u64) -> OutPoint;
pub(crate) fn satpoint(n: u64, offset: u64) -> SatPoint;
pub(crate) fn tx_out(value: u64, address: Address) -> TxOut;

// Test addresses
pub(crate) fn address() -> Address;      // bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4
pub(crate) fn recipient() -> Address;    // tb1q6en7qjxgw4ev8xwx94pzdry6a6ky7wlfeqzunz
pub(crate) fn change(n: u64) -> Address; // change(0), change(1), change(2), change(3)
```

### Test Examples

#### Test 1: Basic Fee Calculation

```rust
#[test]
fn reveal_transaction_pays_fee() {
  let utxos = vec![(outpoint(1), tx_out(20000, address()))];
  let inscription = inscription("text/plain", "ord");
  let commit_address = change(0);
  let reveal_address = recipient();
  let reveal_change = [commit_address, change(1)];

  let batch::Transactions {
    commit_tx,
    reveal_tx,
    ..
  } = batch::Plan {
    satpoint: Some(satpoint(1, 0)),
    parent_info: None,
    inscriptions: vec![inscription],
    destinations: vec![reveal_address],
    commit_fee_rate: FeeRate::try_from(1.0).unwrap(),
    reveal_fee_rate: FeeRate::try_from(1.0).unwrap(),
    no_limit: false,
    reinscribe: false,
    postages: vec![TARGET_POSTAGE],
    mode: batch::Mode::SharedOutput,
    ..default()
  }
  .create_batch_transactions(
    BTreeMap::new(),
    Chain::Mainnet,
    BTreeSet::new(),
    BTreeSet::new(),
    utxos.into_iter().collect(),
    reveal_change,
    change(2),
  )
  .unwrap();

  let fee = Amount::from_sat((1.0 * (reveal_tx.vsize() as f64)).ceil() as u64);

  assert_eq!(
    reveal_tx.output[0].value,
    20000 - fee.to_sat() - (20000 - commit_tx.output[0].value),
  );
}
```

**Validates:** Reveal transaction correctly calculates and pays fees.

#### Test 2: RBF Support

```rust
#[test]
fn inscribe_transactions_opt_in_to_rbf() {
  let utxos = vec![(outpoint(1), tx_out(20000, address()))];
  let inscription = inscription("text/plain", "ord");

  let batch::Transactions { reveal_tx, .. } = batch::Plan {
    satpoint: Some(satpoint(1, 0)),
    inscriptions: vec![inscription],
    destinations: vec![recipient()],
    postages: vec![TARGET_POSTAGE],
    mode: batch::Mode::SharedOutput,
    ..default()
  }
  .create_batch_transactions(
    BTreeMap::new(),
    Chain::Mainnet,
    BTreeSet::new(),
    BTreeSet::new(),
    utxos.into_iter().collect(),
    [change(0), change(1)],
    change(2),
  )
  .unwrap();

  assert!(reveal_tx.input[0].sequence.is_rbf());
}
```

**Validates:** Reveal transactions use RBF-enabled sequence numbers.

#### Test 3: Parent-Child Inscriptions

```rust
#[test]
fn inscribe_with_parent() {
  let parent_satpoint = satpoint(1, 0);
  let parent_info = Some(ParentInfo {
    id: inscription_id(1),
    location: parent_satpoint,
    destination: change(0),
    tx_out: tx_out(10_000, change(0)),
  });

  let child_inscription = Inscription {
    parents: vec![inscription_id(1).value()],
    ..inscription("text/plain", "child")
  };

  let batch::Transactions {
    commit_tx,
    reveal_tx,
    ..
  } = batch::Plan {
    satpoint: Some(satpoint(2, 0)),
    parent_info,
    inscriptions: vec![child_inscription],
    destinations: vec![recipient()],
    postages: vec![TARGET_POSTAGE],
    mode: batch::Mode::SharedOutput,
    ..default()
  }
  .create_batch_transactions(
    BTreeMap::from([(parent_satpoint, vec![inscription_id(1)])]),
    Chain::Mainnet,
    BTreeSet::new(),
    BTreeSet::new(),
    utxos.into_iter().collect(),
    [change(1), change(2)],
    change(3),
  )
  .unwrap();

  // Verify parent input/output
  assert_eq!(reveal_tx.input[0].previous_output, parent_satpoint.outpoint);
  assert_eq!(reveal_tx.output[0].script_pubkey, change(0).script_pubkey());
  assert_eq!(reveal_tx.output[0].value, 10_000);
}
```

**Validates:** Parent inscriptions are properly spent and recreated in reveal transaction.

#### Test 4: Batch Mode - Separate Outputs

```rust
#[test]
fn batch_inscriptions_separate_outputs() {
  let inscriptions = vec![
    inscription("text/plain", "inscription 1"),
    inscription("text/plain", "inscription 2"),
    inscription("text/plain", "inscription 3"),
  ];

  let destinations = vec![recipient(), change(0), change(1)];

  let batch::Transactions { reveal_tx, .. } = batch::Plan {
    satpoint: Some(satpoint(1, 0)),
    inscriptions,
    destinations,
    postages: vec![
      Amount::from_sat(10_000),
      Amount::from_sat(10_000),
      Amount::from_sat(10_000),
    ],
    mode: batch::Mode::SeparateOutputs,
    ..default()
  }
  .create_batch_transactions(/* ... */)
  .unwrap();

  // Each inscription gets its own output
  assert_eq!(reveal_tx.output.len(), 3);
  assert_eq!(reveal_tx.output[0].value, 10_000);
  assert_eq!(reveal_tx.output[1].value, 10_000);
  assert_eq!(reveal_tx.output[2].value, 10_000);
}
```

**Validates:** Separate outputs mode creates one output per inscription.

#### Test 5: UTXO Selection Error

```rust
#[test]
fn no_cardinal_utxos_error() {
  let inscribed_satpoint = satpoint(1, 0);

  let error = batch::Plan {
    satpoint: None,  // Auto-select
    inscriptions: vec![inscription("text/plain", "test")],
    destinations: vec![recipient()],
    ..default()
  }
  .create_batch_transactions(
    BTreeMap::from([(inscribed_satpoint, vec![inscription_id(1)])]),
    Chain::Mainnet,
    BTreeSet::new(),
    BTreeSet::new(),
    BTreeMap::from([(outpoint(1), tx_out(10_000, address()))]),  // Only inscribed UTXO
    [change(0), change(1)],
    change(2),
  )
  .unwrap_err();

  assert!(error.to_string().contains("wallet contains no cardinal utxos"));
}
```

**Validates:** Proper error when no cardinal UTXOs are available.

### Running Tests

```bash
# Run all batch tests
cargo test --test wallet batch

# Run specific test
cargo test --test wallet reveal_transaction_pays_fee

# Run with output
cargo test --test wallet batch -- --nocapture

# Run unit tests in plan.rs
cargo test -p ord --lib wallet::batch::plan
```

## Error Handling

### Common Errors

| Error | Cause | Solution |
|-------|-------|----------|
| `wallet contains no cardinal utxos` | No UTXOs available that aren't inscribed/locked/runic | Add funds or unlock UTXOs |
| `sat at {satpoint} already inscribed` | Attempting to inscribe already-inscribed sat without `--reinscribe` | Use `--reinscribe` flag or choose different sat |
| `reveal transaction weight greater than MAX_STANDARD_TX_WEIGHT` | Inscription data too large | Use `--no-limit` flag (non-standard tx) or reduce data size |
| `runestone greater than maximum OP_RETURN size` | Rune etching data exceeds 82 bytes | Simplify rune parameters or use `--no-limit` |
| `Failed to sign reveal transaction` | Missing prevouts or incorrect key | Verify commitment output and key match |

### Error Recovery

**If reveal fails after commit:**

1. Extract recovery key from output: `rawtr(...)#checksum`
2. Import to wallet: `bitcoin-cli importdescriptors '[{"desc":"rawtr(...)#checksum","timestamp":"now","active":false}]'`
3. Sweep funds: `bitcoin-cli sendtoaddress <your_address> <amount>`

**If using split workflow:**

1. Save `--key` output from commit phase
2. Wait for commitment to confirm (if etching rune)
3. Use same `--key` with `--commitment` for reveal phase

## Performance Considerations

### Complexity

- **Time Complexity:** O(n) where n = number of UTXOs + number of inscriptions
- **Space Complexity:** O(m) where m = total inscription data size

### Bottlenecks

1. **UTXO iteration:** O(n) to find cardinal UTXO
2. **Reveal script construction:** O(m) where m = inscription data size
3. **Signature hash calculation:** O(p) where p = number of prevouts
4. **Witness size estimation:** Two full transaction builds

### Optimizations

- UTXOs stored in BTreeMap for O(log n) lookup
- Reuses reveal script across multiple function calls
- Calculates reveal fee before commit to determine exact target value

## Security Considerations

### Key Security

- Recovery keys generated with cryptographically secure RNG (`rand::thread_rng()`)
- Keys are taproot-tweaked to commit to reveal script
- Private keys never logged (only public descriptors)
- Backup only occurs for auto-generated keys (user-provided keys assumed secure)

### Transaction Security

- Validates all outputs meet dust threshold
- Checks transaction weight limits (400,000 WU standard)
- Verifies signature completeness before broadcast
- Uses RBF for fee bumping capability
- Validates runestone encoding matches expected

### UTXO Security

- Never spends inscribed UTXOs (unless reinscribing)
- Avoids locked UTXOs (prevents double-spend attempts)
- Protects runic UTXOs from accidental spending
- Validates satpoint exists and has sufficient value

## Debugging Tips

### Enable Debug Logging

```rust
// In plan.rs, add println! statements:
println!("Selected satpoint: {:?}", satpoint);
println!("Recovery key: {:?}", recovery_key_pair);
println!("Commit address: {}", commit_tx_address);
println!("Reveal fee: {} sats", reveal_fee);
```

### Inspect Transaction Hex

```rust
println!("Commit hex: {}", commit_tx.raw_hex());
println!("Reveal hex: {}", reveal_tx.raw_hex());
```

### Decode Transactions

```bash
# Decode commit transaction
bitcoin-cli decoderawtransaction <commit_hex>

# Decode reveal transaction
bitcoin-cli decoderawtransaction <reveal_hex>

# Analyze reveal transaction
bitcoin-cli analyzepsbt <reveal_psbt>
```

### Check Signature

```bash
# Verify reveal transaction signature
bitcoin-cli testmempoolaccept '["<reveal_hex>"]'
```

### Trace Key Derivation

```rust
let untweaked_pubkey = XOnlyPublicKey::from_keypair(&key_pair);
println!("Untweaked pubkey: {}", untweaked_pubkey);

let tweaked_keypair = key_pair.tap_tweak(&secp256k1, taproot_spend_info.merkle_root());
let (tweaked_pubkey, _) = tweaked_keypair.to_inner().x_only_public_key();
println!("Tweaked pubkey: {}", tweaked_pubkey);

let address = Address::p2tr_tweaked(
  TweakedPublicKey::dangerous_assume_tweaked(tweaked_pubkey),
  network,
);
println!("Commit address: {}", address);
```

## Related Functions

### Callers

- `Plan::inscribe()` (plan.rs:67-237) - Main orchestrator, calls `create_batch_transactions()` and handles broadcasting

### Dependencies

- `TransactionBuilder::build_transaction()` - Builds commit transaction
- `Inscription::append_batch_reveal_script()` - Constructs reveal script with inscriptions
- `Runestone::encipher()` - Encodes runestone to OP_RETURN
- `Runestone::decipher()` - Validates runestone encoding

## See Also

- [Bitcoin Taproot BIP 340-342](https://github.com/bitcoin/bips)
- [Ordinals Theory](https://docs.ordinals.com/overview.html)
- [Runes Protocol](https://docs.ordinals.com/runes.html)
- [Bitcoin Script Reference](https://en.bitcoin.it/wiki/Script)

---

**Last Updated:** Based on `ft-recovery-key` branch analysis (2024)

**Maintainer:** ord development team

**License:** See repository LICENSE file
