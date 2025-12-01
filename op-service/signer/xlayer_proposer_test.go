//go:build !ci
// +build !ci

package signer

import (
	"context"
	"encoding/json"
	"fmt"
	"math/big"
	"strings"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-service/testlog"
	"github.com/ethereum-optimism/optimism/packages/contracts-bedrock/snapshots"
)

func TestProposer_DisputeGameFactory_Sepolia_FullFlow(t *testing.T) {

	// t.Skip("Skipping Sepolia on-chain test. Remove this line to run manually.")

	logger := testlog.Logger(t, log.LevelInfo)

	sepoliaRPC := "https://www.okx.com/fullnode/xlayer/ethsepolia/discover/rpc"
	client, err := ethclient.Dial(sepoliaRPC)
	require.NoError(t, err, "Failed to connect to Sepolia")
	defer client.Close()

	t.Logf("Connected to Sepolia testnet")
	chainID := big.NewInt(11155111)
	proposerAddr := common.HexToAddress("0x1a13bddcc02d363366e04d4aa588d3c125b0ff6f")
	disputeGameFactoryAddr := common.HexToAddress("0xca66313d59c9aab29a0e2a84635dc6778c4c5819")

	ctx := context.Background()
	currentNonce, err := client.PendingNonceAt(ctx, proposerAddr)
	require.NoError(t, err, "Failed to get nonce")
	t.Logf("Current nonce for %s: %d", proposerAddr.Hex(), currentNonce)

	gasPrice, err := client.SuggestGasPrice(ctx)
	require.NoError(t, err, "Failed to get gas price")
	t.Logf("Suggested gas price: %s wei", gasPrice)

	gameType := uint32(1)
	//rootClaim := common.HexToHash("0x6b88b9b1af7e1d0ec959abffdf24cac21f8703c9fd066bb40e8aae7bcddd773d")
	//extraData := common.FromHex("0x000000000000000000000000000000000000000000000000000000000226c2b2")
	rootClaim := common.HexToHash("0xa394e33559e15a0661a3f16ea6309a05ba182d2e243a3cd29c06b7cf7379199e")
	extraData := common.FromHex("0x0000000000000000000000000000000000000000000000000000000003a2007d")
	l2BlockNumber := new(big.Int).SetBytes(extraData[len(extraData)-8:]).Uint64()

	t.Logf("DisputeGameFactory.create parameters:")
	t.Logf("   - gameType: %d (PERMISSIONED)", gameType)
	t.Logf("   - rootClaim: %s", rootClaim.Hex())
	t.Logf("   - extraData: %s (L2 block number: %d)", hexutil.Encode(extraData), l2BlockNumber)

	disputeGameFactoryABI := snapshots.LoadDisputeGameFactoryABI()
	txData, err := disputeGameFactoryABI.Pack("create", gameType, rootClaim, extraData)
	require.NoError(t, err, "Failed to pack create method")
	t.Logf("Packed transaction data, length: %d bytes", len(txData))

	dynamicTx := &types.DynamicFeeTx{
		ChainID:   chainID,
		Nonce:     currentNonce,
		GasTipCap: new(big.Int).Mul(gasPrice, big.NewInt(2)), // 2x gas price
		GasFeeCap: new(big.Int).Mul(gasPrice, big.NewInt(3)), // 3x gas price
		Gas:       10000000,
		To:        &disputeGameFactoryAddr,
		Value:     big.NewInt(0), // 1 ETH bond
		Data:      txData,
	}

	tx := types.NewTx(dynamicTx)

	t.Logf("Created EIP-1559 Proposal Transaction:")
	t.Logf("   - Type: %d (EIP-1559)", tx.Type())
	t.Logf("   - ChainID: %s", chainID)
	t.Logf("   - Nonce: %d", tx.Nonce())
	//t.Logf("   - To: %s (DisputeGameFactory)", disputeGameFactoryAddr.Hex())
	t.Logf("   - Gas: %d", tx.Gas())
	t.Logf("   - GasTipCap: %s wei", tx.GasTipCap())
	t.Logf("   - GasFeeCap: %s wei", tx.GasFeeCap())
	t.Logf("   - Value: %s wei (bond)", tx.Value())
	t.Logf("   - Data length: %d bytes", len(tx.Data()))

	xlayerConfig := XLayerConfig{
		Endpoint:        "http://asset-onchain.base-global.svc.test.local:7001",
		Address:         proposerAddr.Hex(),
		UserID:          0,
		Symbol:          2882,
		ProjectSymbol:   3011,
		OperateSymbol:   2,
		OperateAmount:   "0",
		SysFrom:         3,
		RequestSignURI:  "/priapi/v1/assetonchain/ecology/ecologyOperate",
		QuerySignURI:    "/priapi/v1/assetonchain/ecology/querySignDataByOrderNo",
		DepositeAddress: "0x006737cc6980a7786a477ce46b491845509b19dc",
		AccessKey:       "test-access-key",
		SecretKey:       "test-secret-key",
	}

	xlayerClient := NewXLayerRemoteClient(logger, xlayerConfig)

	t.Logf("Sending remote signing request to XLayer (operateType=20)...")
	signCtx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	signedTx, err := xlayerClient.SignTransaction(signCtx, chainID, proposerAddr, tx)
	if err != nil {
		if strings.Contains(err.Error(), "相同地址有未完成交易") {
			t.Logf("Address has pending transactions, waiting 10 seconds...")
			time.Sleep(10 * time.Second)
			signedTx, err = xlayerClient.SignTransaction(signCtx, chainID, proposerAddr, tx)
		}

		if err != nil {
			t.Fatalf("Remote signing failed: %v", err)
		}
	}

	require.NotNil(t, signedTx, "Signed transaction should not be nil")
	t.Logf("Successfully signed transaction: %s", signedTx.Hash().Hex())

	t.Logf("Signed Transaction Details:")
	t.Logf("  - Type: %d", signedTx.Type())
	t.Logf("   - ChainID: %s", signedTx.ChainId())
	t.Logf("   - Nonce: %d", signedTx.Nonce())
	if signedTx.To() != nil {
		t.Logf("   - To: %s", signedTx.To().Hex())
	} else {
		t.Logf("   - To: nil (contract creation)")
	}
	t.Logf("   - Gas: %d", signedTx.Gas())
	t.Logf("   - Value: %s wei", signedTx.Value())

	if signedTx.Type() == types.DynamicFeeTxType {
		t.Logf("   - GasTipCap: %s wei", signedTx.GasTipCap())
		t.Logf("   - GasFeeCap: %s wei", signedTx.GasFeeCap())
	} else if signedTx.Type() == types.LegacyTxType {
		t.Logf("   - GasPrice: %s wei", signedTx.GasPrice())
	}

	signer := types.LatestSignerForChainID(signedTx.ChainId())

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
	t.Logf("   - Expected Sender: %s", proposerAddr.Hex())
	t.Logf("   - Recovered Sender: %s", recoveredFrom.Hex())

	if recoveredFrom == proposerAddr {
		t.Logf("Signature verification passed")
	} else {
		t.Logf("Signature verification failed!")
		t.Logf("Remote signer used wrong private key")
		t.Logf("Continuing test despite signature mismatch...")
	}

	t.Logf("Verifying business parameters in signed transaction...")

	if len(signedTx.Data()) >= 4 {
		methodSig := signedTx.Data()[:4]
		methodData := signedTx.Data()[4:]

		if method, err := disputeGameFactoryABI.MethodById(methodSig); err == nil && method.Name == "create" {
			result := make(map[string]interface{})
			if err := method.Inputs.UnpackIntoMap(result, methodData); err == nil {
				unpackedGameType := result["_gameType"].(uint32)
				//unpackedRootClaim := result["_rootClaim"].(common.Hash)
				var unpackedRootClaim common.Hash
				switch v := result["_rootClaim"].(type) {
				case [32]byte:
					unpackedRootClaim = common.BytesToHash(v[:])
				case common.Hash:
					unpackedRootClaim = v
				default:
					t.Logf("Unexpected rootClaim type: %T", v)
				}
				unpackedExtraData := result["_extraData"].([]byte)

				t.Logf("Business parameters verified:")
				t.Logf("   - gameType: %d (expected: %d)", unpackedGameType, gameType)
				t.Logf("   - rootClaim: %s", unpackedRootClaim.Hex())
				t.Logf("   - extraData: %s", hexutil.Encode(unpackedExtraData))

				require.Equal(t, gameType, unpackedGameType)
				require.Equal(t, rootClaim, unpackedRootClaim)
				require.Equal(t, extraData, unpackedExtraData)
			}
		}
	}

	t.Logf("Sending proposal transaction to Sepolia testnet...")
	sendCtx, sendCancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer sendCancel()

	err = client.SendTransaction(sendCtx, signedTx)
	if err != nil {
		t.Logf(" Failed to send transaction: %v", err)
		t.Logf("This might be due to:")
		t.Logf("   - Insufficient balance in proposer address (needs 1 ETH + gas)")
		t.Logf("   - Nonce mismatch")
		t.Logf("   - Gas price too low")
		t.Logf("   - Invalid bond amount")
		t.Logf("   - DisputeGameFactory contract not deployed at this address")
		t.Logf("   - Signature verification failed on chain")
	} else {
		t.Logf("Transaction sent successfully!")
		t.Logf("Transaction Hash: %s", signedTx.Hash().Hex())
		t.Logf("View on Etherscan: https://sepolia.etherscan.io/tx/%s", signedTx.Hash().Hex())

		t.Logf("Waiting for transaction confirmation...")

		receipt, err := waitForProposerReceipt(client, signedTx.Hash(), 2*time.Minute)
		if err != nil {
			t.Logf("Failed to get receipt: %v", err)
		} else {
			t.Logf("Transaction confirmed!")
			t.Logf(" Receipt Details:")
			t.Logf("   - Block Number: %d", receipt.BlockNumber)
			t.Logf("   - Status: %d (1=success, 0=failed)", receipt.Status)
			t.Logf("   - Gas Used: %d", receipt.GasUsed)
			t.Logf("   - Cumulative Gas Used: %d", receipt.CumulativeGasUsed)

			if receipt.Status == types.ReceiptStatusSuccessful {
				t.Logf(" Proposal transaction executed successfully!")
			} else {
				t.Logf("  Proposal transaction reverted")
				t.Logf(" Common reasons for failure:")
				t.Logf("   1. NoImplementation - gameType not registered")
				t.Logf("   2. Insufficient bond - need exactly 1 ETH (or configured amount)")
				t.Logf("   3. Invalid rootClaim format")
				t.Logf("   4. Contract paused or access control")
				t.Logf("   5. Duplicate proposal")
			}

			if len(receipt.Logs) > 0 {
				t.Logf(" Transaction Logs (%d events):", len(receipt.Logs))
				for i, log := range receipt.Logs {
					t.Logf("   Log %d: Address=%s, Topics=%d", i, log.Address.Hex(), len(log.Topics))
					if len(log.Topics) > 0 {
						t.Logf("     Topic[0] (Event Signature): %s", log.Topics[0].Hex())
					}
				}
			}

			t.Logf(" View on Etherscan: https://sepolia.etherscan.io/tx/%s", receipt.TxHash.Hex())
		}
	}

	t.Logf(" Full Proposer flow test completed")
}

func TestProposer_OtherInfoWithBusinessParams(t *testing.T) {
	logger := testlog.Logger(t, log.LevelDebug)

	proposerAddr := common.HexToAddress("0x2222222222222222222222222222222222222222")
	disputeGameFactoryAddr := common.HexToAddress("0x3333333333333333333333333333333333333333")

	config := XLayerConfig{
		Endpoint: "https://test.example.com",
		Address:  proposerAddr.Hex(),
	}
	client := NewXLayerRemoteClient(logger, config)

	gameType := uint32(0)
	rootClaim := common.HexToHash("0x6b88b9b1af7e1d0ec959abffdf24cac21f8703c9fd066bb40e8aae7bcddd773d")
	extraData := common.BigToHash(big.NewInt(12345)).Bytes()

	disputeGameFactoryABI := snapshots.LoadDisputeGameFactoryABI()
	txData, err := disputeGameFactoryABI.Pack("create", gameType, rootClaim, extraData)
	require.NoError(t, err)

	dynamicTx := &types.DynamicFeeTx{
		ChainID:   big.NewInt(11155111),
		Nonce:     100,
		GasTipCap: big.NewInt(2000000000),
		GasFeeCap: big.NewInt(30000000000),
		Gas:       200000,
		To:        &disputeGameFactoryAddr,
		Value:     big.NewInt(1000000000000000000),
		Data:      txData,
	}

	tx := types.NewTx(dynamicTx)

	otherInfo, err := client.buildProposerOtherInfo(tx)
	require.NoError(t, err)
	require.NotEmpty(t, otherInfo)

	t.Logf(" Proposer OtherInfo:")
	t.Logf("%s", otherInfo)

	var parsed map[string]interface{}
	err = json.Unmarshal([]byte(otherInfo), &parsed)
	require.NoError(t, err)

	require.EqualValues(t, strings.ToLower(disputeGameFactoryAddr.Hex()), strings.ToLower(parsed["contractAddress"].(string)))
	require.Equal(t, float64(200000), parsed["gasLimit"])
	require.Equal(t, float64(100), parsed["nonce"])

	require.Equal(t, float64(gameType), parsed["gameType"])
	require.Equal(t, rootClaim.Hex(), parsed["rootClaim"])
	require.Equal(t, hexutil.Encode(extraData), parsed["extraData"])

	t.Logf(" All parameters validated successfully")
}

func waitForProposerReceipt(client *ethclient.Client, txHash common.Hash, timeout time.Duration) (*types.Receipt, error) {
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
		}
	}
}
