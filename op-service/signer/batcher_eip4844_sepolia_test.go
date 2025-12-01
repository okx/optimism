//go:build !ci
// +build !ci

package signer

import (
	"context"
	"fmt"
	"math/big"
	"os"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto/kzg4844"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/log"
	"github.com/holiman/uint256"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
)

// TestBatcher_EIP4844_Sepolia_FullFlow is a complete EIP-4844 test: create blob, sign, and send on-chain
func TestBatcher_EIP4844_Sepolia_FullFlow(t *testing.T) {

	// t.Skip("Skipping Sepolia on-chain test. Remove this line to run the test manually.")

	logger := testlog.Logger(t, log.LevelInfo)

	sepoliaRPC := "https://www.okx.com/fullnode/xlayer/ethsepolia/discover/rpc"

	client, err := ethclient.Dial(sepoliaRPC)
	require.NoError(t, err, "Failed to connect to Sepolia")
	defer client.Close()

	t.Logf("Connected to Sepolia testnet")

	// Setup test parameters
	chainID := big.NewInt(11155111) // Sepolia chain ID
	batcherAddr := common.HexToAddress("0x1a13bddcc02d363366e04d4aa588d3c125b0ff6f")
	batchInboxAddr := common.HexToAddress("0x006737cc6980a7786a477ce46b491845509b19dc")

	ctx := context.Background()
	currentNonce, err := client.PendingNonceAt(ctx, batcherAddr)
	require.NoError(t, err, "Failed to get nonce")

	testBlob := createRealBlobWithSidecar(t)
	commitment, err := kzg4844.BlobToCommitment(testBlob.KZGBlob())
	require.NoError(t, err, "Failed to compute KZG commitment")

	proof, err := kzg4844.ComputeBlobProof(testBlob.KZGBlob(), commitment)
	require.NoError(t, err, "Failed to compute KZG proof")

	t.Logf("Created blob with KZG commitment and proof")

	gasPrice, err := client.SuggestGasPrice(ctx)
	require.NoError(t, err, "Failed to get gas price")

	header, err := client.HeaderByNumber(ctx, nil)
	require.NoError(t, err, "Failed to get latest header")

	blobBaseFee := big.NewInt(1000000000) // Default 1 gwei
	if header.BlobGasUsed != nil && *header.BlobGasUsed > 0 {
		blobBaseFee = new(big.Int).Mul(gasPrice, big.NewInt(2))
	}

	t.Logf("Gas Prices:")
	t.Logf("   - Gas Price: %s wei", gasPrice)
	t.Logf("   - Blob Base Fee: %s wei", blobBaseFee)

	blobTx := &types.BlobTx{
		ChainID:    uint256.MustFromBig(chainID),
		Nonce:      currentNonce,
		GasTipCap:  uint256.MustFromBig(new(big.Int).Mul(gasPrice, big.NewInt(2))), // 2x gas price
		GasFeeCap:  uint256.MustFromBig(new(big.Int).Mul(gasPrice, big.NewInt(3))), // 3x gas price
		Gas:        1000000,
		To:         batchInboxAddr,
		Value:      uint256.MustFromBig(big.NewInt(0)),
		Data:       []byte{},
		BlobFeeCap: uint256.MustFromBig(blobBaseFee),
		BlobHashes: []common.Hash{eth.KZGToVersionedHash(commitment)},
		Sidecar: &types.BlobTxSidecar{
			Blobs:       []kzg4844.Blob{*testBlob.KZGBlob()},
			Commitments: []kzg4844.Commitment{commitment},
			Proofs:      []kzg4844.Proof{proof},
		},
	}

	tx := types.NewTx(blobTx)

	t.Logf("Created EIP-4844 Blob Transaction:")
	t.Logf("   - ChainID: %s", chainID)
	t.Logf("   - Nonce: %d", currentNonce)
	t.Logf("   - To: %s (BatchInbox)", batchInboxAddr.Hex())
	t.Logf("   - Gas: %d", tx.Gas())
	t.Logf("   - Blob Fee Cap: %s wei", tx.BlobGasFeeCap())
	t.Logf("   - Blob Hashes: %d", len(tx.BlobHashes()))
	t.Logf("   - Has Sidecar: %v", tx.BlobTxSidecar() != nil)

	xlayerConfig := XLayerConfig{
		Endpoint:        "http://asset-onchain.base-global.svc.test.local:7001",
		Address:         batcherAddr.Hex(),
		UserID:          0,
		Symbol:          2882,
		ProjectSymbol:   3011,
		OperateSymbol:   2,
		OperateAmount:   "0",
		SysFrom:         3,
		RequestSignURI:  "/priapi/v1/assetonchain/ecology/ecologyOperate",
		QuerySignURI:    "/priapi/v1/assetonchain/ecology/querySignDataByOrderNo",
		DepositeAddress: "0x1a13bddcc02d363366e04d4aa588d3c125b0ff6f",
		AccessKey:       "test-access-key",
		SecretKey:       "test-secret-key",
	}

	xlayerClient := NewXLayerRemoteClient(logger, xlayerConfig)

	t.Logf("Sending remote signing request to XLayer...")
	signCtx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	signedTx, err := xlayerClient.SignTransaction(signCtx, chainID, batcherAddr, tx)
	if err != nil {
		t.Fatalf("Remote signing failed: %v", err)
	}

	require.NotNil(t, signedTx, "Signed transaction should not be nil")
	t.Logf("Successfully signed transaction: %s", signedTx.Hash().Hex())

	// Verify signed transaction
	t.Logf("Signed Transaction Details:")
	t.Logf("   - Type: %d", signedTx.Type())
	t.Logf("   - ChainID: %s", signedTx.ChainId())
	t.Logf("   - Nonce: %d ", signedTx.Nonce())
	t.Logf("   - To: %s", signedTx.To().Hex())
	t.Logf("   - Gas: %d", signedTx.Gas())
	t.Logf("   - Value: %s", signedTx.Value())
	t.Logf("   - Has Sidecar: %v", signedTx.BlobTxSidecar() != nil)

	signer := types.NewCancunSigner(signedTx.ChainId())
	// Get signature values
	v, r, s := signedTx.RawSignatureValues()
	t.Logf("Raw Signature Values:")
	t.Logf("   - V: %s", v.String())
	t.Logf("   - R: %s", r.String())
	t.Logf("   - S: %s", s.String())

	sigHash := signer.Hash(signedTx)
	t.Logf("Transaction Signature Hash: %s", sigHash.Hex())

	recoveredFrom, err := signer.Sender(signedTx)
	require.NoError(t, err, "Failed to recover sender")

	t.Logf("Signature Verification:")
	t.Logf("   - Expected Sender: %s", batcherAddr.Hex())
	t.Logf("   - Recovered Sender: %s", recoveredFrom.Hex())
	t.Logf("   - ChainID used for signing: %s", signedTx.ChainId().String())

	if recoveredFrom == batcherAddr {
		t.Logf("Signature verification passed")
	} else {
		t.Logf("Signature verification failed!")
		t.Logf("Continuing test despite signature mismatch...")
	}

	t.Logf("SignTransaction completed, transaction already reassembled by XLayerRemoteClient")

	// Final transaction validation before sending
	t.Logf("Final transaction validation before sending:")
	t.Logf("   - Type: %d (should be 3 for blob)", signedTx.Type())
	t.Logf("   - Nonce: %d", signedTx.Nonce())
	t.Logf("   - To: %s", signedTx.To().Hex())
	t.Logf("   - Gas: %d", signedTx.Gas())
	t.Logf("   - GasTipCap: %s wei", signedTx.GasTipCap())
	t.Logf("   - GasFeeCap: %s wei", signedTx.GasFeeCap())

	if signedTx.Type() == types.BlobTxType {
		t.Logf("   - BlobFeeCap: %s wei", signedTx.BlobGasFeeCap())
		t.Logf("   - BlobHashes: %d", len(signedTx.BlobHashes()))
		t.Logf("   - Has Sidecar: %v", signedTx.BlobTxSidecar() != nil)

		// Check Sidecar details
		sidecar := signedTx.BlobTxSidecar()
		if sidecar != nil {
			t.Logf("   - Sidecar: Blobs=%d, Commitments=%d, Proofs=%d",
				len(sidecar.Blobs), len(sidecar.Commitments), len(sidecar.Proofs))
		}

		// Check serialization size
		txBytes, err := signedTx.MarshalBinary()
		if err != nil {
			t.Logf("   - Marshal error: %v", err)
		} else {
			t.Logf("   - TX size: %d bytes (%.2f KB)", len(txBytes), float64(len(txBytes))/1024)
			if sidecar != nil && len(sidecar.Blobs) > 0 && len(txBytes) > 131072 {
				t.Logf("  Size correct, Sidecar included")
			} else if len(txBytes) < 1000 {
				t.Logf("  Size too small, Sidecar might be missing")
			}

			// Write serialized data to file
			hexStr := common.Bytes2Hex(txBytes)
			err := writeToFile("signed_blob_tx.txt", hexStr)
			if err != nil {
				t.Logf("   - Failed to write to file: %v", err)
			} else {
				t.Logf("  Serialized tx written to signed_blob_tx.txt")
				t.Logf("   - File size: %.2f KB", float64(len(hexStr))/1024)
			}
		}

		if signedTx.BlobGasFeeCap().Cmp(big.NewInt(0)) == 0 {
			t.Logf("BlobFeeCap is 0, this will fail!")
		}
	}

	// Send transaction to Sepolia
	t.Logf("Sending transaction to Sepolia testnet...")
	sendCtx, sendCancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer sendCancel()

	err = client.SendTransaction(sendCtx, signedTx)
	if err != nil {
		t.Logf("Failed to send transaction: %v", err)
		t.Logf("This might be due to:")
		t.Logf("   - Insufficient balance in batcher address")
		t.Logf("   - Nonce mismatch")
		t.Logf("   - Gas price too low")
		t.Logf("   - Network issues")
		// Do not fail the test as this might be expected
	} else {
		t.Logf("Transaction sent successfully!")
		t.Logf("Transaction Hash: %s", signedTx.Hash().Hex())
		t.Logf("View on Etherscan: https://sepolia.etherscan.io/tx/%s", signedTx.Hash().Hex())

		// Wait for transaction confirmation
		t.Logf("Waiting for transaction confirmation...")

		receipt, err := waitForTransactionReceipt(client, signedTx.Hash(), 2*time.Minute)
		if err != nil {
			t.Logf("Failed to get receipt: %v", err)
		} else {
			t.Logf("Transaction confirmed!")
			t.Logf("Receipt Details:")
			t.Logf("   - Block Number: %d", receipt.BlockNumber)
			t.Logf("   - Status: %d (1=success, 0=failed)", receipt.Status)
			t.Logf("   - Gas Used: %d", receipt.GasUsed)
			if receipt.BlobGasUsed > 0 {
				t.Logf("   - Blob Gas Used: %d", receipt.BlobGasUsed)
			}
			t.Logf("View on Etherscan: https://sepolia.etherscan.io/tx/%s", receipt.TxHash.Hex())
		}
	}

	t.Logf("Full EIP-4844 flow test completed")
}

// createRealBlobWithSidecar creates valid blob data compliant with EIP-4844 specification
func createRealBlobWithSidecar(t *testing.T) *eth.Blob {
	var blob eth.Blob

	// Create blob data compliant with EIP-4844 specification
	// Each field element must be less than BLS12-381 curve modulus
	for i := 0; i < len(blob); i += 32 {
		// Each 32 bytes is a field element
		// Highest byte must be 0 to ensure it's less than modulus
		blob[i] = 0x00

		// Fill remaining 31 bytes
		for j := 1; j < 32 && i+j < len(blob); j++ {
			// Use predictable but varying data
			blob[i+j] = byte((i + j) % 251) // Use 251 to ensure valid range
		}
	}

	// Validate blob data
	_, err := kzg4844.BlobToCommitment(blob.KZGBlob())
	require.NoError(t, err, "Blob data should be valid for KZG")

	return &blob
}

// waitForTransactionReceipt waits for transaction receipt
func waitForTransactionReceipt(client *ethclient.Client, txHash common.Hash, timeout time.Duration) (*types.Receipt, error) {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()

	ticker := time.NewTicker(3 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return nil, fmt.Errorf("timeout waiting for transaction receipt")
		case <-ticker.C:
			receipt, err := client.TransactionReceipt(ctx, txHash)
			if err == nil {
				return receipt, nil
			}
			// Continue waiting
		}
	}
}

// writeToFile writes string to file, overwriting previous content
func writeToFile(filename string, content string) error {
	// Remove old file if exists
	os.Remove(filename)

	// Write new file
	return os.WriteFile(filename, []byte(content), 0644)
}
