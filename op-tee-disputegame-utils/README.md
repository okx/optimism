# op-tee-utils

This crate provides two standalone binaries for submitting AWS Nitro attestation document to the [Boundless](https://boundless.network) decentralized proving market.
The successful submission will return the zkproof of the attestation document. 
And the binary will process the zkproof to generate calldata to invoke [`TeeProofVerifier.register()`](../packages/contracts-bedrock/src/dispute//tee/TeeProofVerifier.sol), which will verify the zkproof and register the TEE's EOA in the zkproof to contract. Refer to [doc](https://okg-block.sg.larksuite.com/wiki/BGFJwOzKOiY6FHkPajylRXbpgSe) for more details. 

| Binary | Purpose |
|---|---|
| `upload_program` | Upload the guest ELF to IPFS once and obtain a permanent `PROGRAM_URL` + `IMAGE_ID` |
| `request_proof` | Submit a proof request for an attestation document and retrieve the resulting seal + journal |

The typical workflow is:
1. Run `upload_program` **once** to publish the guest program.
2. Run `request_proof` as many times as needed, reusing the `PROGRAM_URL` and `IMAGE_ID` from step 1.

---

## Preliminary: Preparing Parameters

Both binaries read configuration exclusively from environment variables — no `.env` file loading.

### `PRIVATE_KEY`

A `0x`-prefixed hex private key for an Ethereum-compatible wallet.

The wallet must hold **at least ~10 USD worth of ETH on the Base network** to cover gas fees for submitting proof requests on-chain.

```
PRIVATE_KEY=0x<64 hex chars>
```

> Keep this value secret. Never commit it to version control.

### `RPC_URL`

An RPC endpoint for the **Base** network, where the Boundless Market contract is deployed.

Free public options:
- `https://mainnet.base.org`
- `https://base-rpc.publicnode.com`

```
RPC_URL=https://mainnet.base.org
```

### `PINATA_JWT`

A Pinata API JWT used to upload the guest ELF to IPFS.

Follow the [Boundless storage-uploader tutorial](https://docs.boundless.network/developers/tutorials/request#storage-uploader) to create a Pinata account and generate a JWT.

```
PINATA_JWT=eyJhbGci...
```

---

## Step 1 — Install RISC Zero (required for `upload_program` only)

`upload_program` embeds the guest ELF at compile time, which requires the RISC Zero toolchain.

> **This is a one-time setup.** Once you have a `PROGRAM_URL` and `IMAGE_ID`, you can run `request_proof` on any machine without RISC Zero installed.

Run the following two commands to install:

```bash
curl -L https://risczero.com/install | bash
rzup install
```

For full details see the [Boundless quick-start guide](https://docs.boundless.network/developers/quick-start#install-risc-zero).

---

## Step 2 — Upload the Guest Program (once)

> **Docker is required.** The RISC Zero build system uses Docker to compile the guest ELF reproducibly. Make sure the Docker daemon is running before proceeding.

Run from the `op-tee-disputegame-utils/` directory:

```bash
cd op-tee-disputegame-utils

RPC_URL=https://mainnet.base.org \
PRIVATE_KEY=0x<your-key> \
PINATA_JWT=<your-jwt> \
  cargo run --release -p prover --features embed-methods --bin upload_program
```

The binary prints to stdout:

```
PROGRAM_URL=https://gateway.pinata.cloud/ipfs/Qm...
IMAGE_ID=0x...
```

To persist the output, pass a file path as a positional argument:

```bash
RPC_URL=https://mainnet.base.org \
PRIVATE_KEY=0x<your-key> \
PINATA_JWT=<your-jwt> \
  cargo run --release -p prover --features embed-methods --bin upload_program \
  -- program_info.json
```

`program_info.json` will contain:

```json
{
  "program_url": "https://gateway.pinata.cloud/ipfs/Qm...",
  "image_id": "0x..."
}
```

> **You only need to run this once.** Re-uploading the same ELF wastes Pinata storage and gas fees. Save the `PROGRAM_URL` and `IMAGE_ID` values — you will reuse them every time you call `request_proof`.

> **401 Unauthorized?** If the upload fails with a 401 status, your `PINATA_JWT` has expired or been revoked. Regenerate a new JWT from the [Pinata API Keys page](https://app.pinata.cloud/developers/api-keys) and update the `PINATA_JWT` env var before retrying.

---

## Step 3 — Request a Proof

> **Cost per proof:** Each proof request costs approximately **~0.05 USD worth of ETH** on the Base network (gas + prover fee).

### Context: TeeDisputeGame Integration

This step is the core of the TEE attestation workflow. When an AWS Nitro Enclave launches, it produces a **Nitro attestation document** that cryptographically binds the Enclave's PCR measurements (PCR0/1/2) to a freshly generated **TEE EOA public key** (secp256k1). Pass this attestation document to `request_proof` to generate an on-chain-verifiable Groth16 ZK proof.

The two proof outputs are:

| Output | Description |
|---|---|
| **`journal`** (public inputs) | Raw bytes encoding: attestation timestamp, SHA256(PCR0), root CA public key, enclave public key, and user data. These are the publicly verifiable claims passed to `register()` as `AttestationData`. |
| **`seal`** (Groth16 proof) | Compact SNARK proof that the journal was derived from a valid Nitro attestation signed by the AWS root key — without revealing the full attestation document. |

`seal` + `journal` form the calldata submitted to the **`TeeVerifier` contract**. On successful on-chain verification, the contract registers the PCR hash and TEE EOA public key — making that Enclave instance a trusted attesting party recognized by the **`TeeDisputeGame` contract**.

> For the full `TeeVerifier` / `TeeDisputeGame` contract interface, see the [contract documentation](https://okg-block.sg.larksuite.com/wiki/BGFJwOzKOiY6FHkPajylRXbpgSe).

---

Run from the `op-tee-disputegame-utils/` directory:

```bash
cd op-tee-disputegame-utils

RPC_URL=https://mainnet.base.org \
PRIVATE_KEY=0x<your-key> \
PINATA_JWT=<your-jwt> \
  cargo run --release -p prover --bin request_proof -- \
    --attestation "$attest_doc" \   # Note: $attest_doc should be the base64-encoded Nitro attestation document
    --program-info program_info.json \
    --output ./proof_result.jsocargo run --release -p prover --bin request_proof -- \
    --attestation "$attest_doc" \   # Note: $attest_doc should be the base64-encoded Nitro attestation document
    --program-info program_info.json \
    --output ./proof_result.jsonn
```

### Flags

| Flag | Description |
|---|---|
| `--attestation`, `-a` | **(Required)** Base64-encoded AWS Nitro attestation document |
| `--program-info`, `-p` | Path to the JSON file produced by `upload_program` |
| `--output`, `-o` | Path to write the proof result JSON |

### Overriding program info via env vars

`PROGRAM_URL` and `IMAGE_ID` env vars take precedence over values in `--program-info`:

```bash
PROGRAM_URL=https://gateway.pinata.cloud/ipfs/Qm... \
IMAGE_ID=0x... \
RPC_URL=https://mainnet.base.org \
PRIVATE_KEY=0x<your-key> \
PINATA_JWT=<your-jwt> \
  cargo run --release -p prover --bin request_proof -- \
    --attestation "$attest_doc" \
    --program-info program_info.json \
    --output ./proof_result.json
```

### Output

Progress and status messages are written to **stderr**. The following lines are written to **stdout**:

```
seal=0x<hex>
journal=0x<hex>
register_calldata=0x<hex>
```

If `--output` is provided, the result JSON will contain:

```json
{
  "seal_hex": "...",
  "journal_hex": "...",
  "image_id_hex": "...",
  "request_id": "...",
  "register_calldata": "0x...",
  "journal_parsed": {
    "timestamp_ms": 1234567890000,
    "pcr0_hash": "0x<hex>",
    "root_pubkey": "0x<hex>",
    "enclave_pubkey": "0x<hex>",
    "user_data": "0x<hex>"
  }
}
```

Proof fulfillment typically takes **5–15 minutes** depending on prover availability and pricing.

### Step 4 — Register the TEE EOA on-chain

> **Action required.** After obtaining a proof, you must call the `TeeVerifier` contract with the `register_calldata` to submit the ZK proof on-chain and register the TEE's Ethereum address (EOA).

The `register_calldata` encodes a call to the following Solidity function:

```solidity
function register(bytes calldata seal, AttestationData calldata attestationData) external onlyOwner;
```

Send the calldata to the deployed `TeeVerifier` contract using your preferred method:

**Using `cast` (Foundry):**

```bash
cast send <TEE_VERIFIER_CONTRACT_ADDRESS> \
  "$(cat proof_result.json | jq -r .register_calldata)" \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY
```

**Using raw calldata:**

```bash
cast send <TEE_VERIFIER_CONTRACT_ADDRESS> \
  <register_calldata printed above> \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY
```

Once the transaction is confirmed, the TEE EOA derived from the attestation document is registered on-chain and trusted by the `TEEDisputeGame` contract.

---

## Optional Environment Variables

| Variable | Default | Description |
|---|---|---|
| `MAX_PRICE_PER_CYCLE` | `50000` | Maximum price per cycle offered to provers (wei) |
| `MIN_PRICE_PER_CYCLE` | `10000` | Minimum price per cycle offered to provers (wei) |
| `POLL_INTERVAL_SECS` | `60` | Seconds between fulfillment polling attempts |
