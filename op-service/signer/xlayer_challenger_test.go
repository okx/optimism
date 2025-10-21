package signer

import (
	"context"
	"fmt"
	"math/big"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-service/testlog"
)

// TestChallenger_ResolveClaim_Sepolia_FullFlow
func TestChallenger_ResolveClaim_Sepolia_FullFlow(t *testing.T) {
	// t.Skip("Skipping Sepolia on-chain test. Remove this line to run manually.")

	logger := testlog.Logger(t, log.LevelInfo)

	sepoliaRPC := "https://www.okx.com/fullnode/xlayer/ethsepolia/discover/rpc"
	//sepoliaRPC := "https://sepolia.infura.io/v3/464a484737734f7db0ef5114b0817d81"
	client, err := ethclient.Dial(sepoliaRPC)
	require.NoError(t, err, "Failed to connect to Sepolia")
	defer client.Close()

	t.Logf("Connected to Sepolia testnet")

	chainID := big.NewInt(11155111)
	challengerAddr := common.HexToAddress("0x1a13bddcc02d363366e04d4aa588d3c125b0ff6f")
	disputeGameAddr := common.HexToAddress("0x336641134aeed9fa85e2caa145bfc3fbc234fd08")

	ctx := context.Background()
	currentNonce, err := client.PendingNonceAt(ctx, challengerAddr)
	require.NoError(t, err, "Failed to get nonce")
	t.Logf("Current nonce for %s: %d", challengerAddr.Hex(), currentNonce)

	gasPrice, err := client.SuggestGasPrice(ctx)
	require.NoError(t, err, "Failed to get gas price")
	t.Logf("Suggested gas price: %s wei", gasPrice)

	t.Logf(" Checking DisputeGame state...")
	t.Logf("   ️ IMPORTANT: resolveClaim requires game to be resolved first!")
	t.Logf("   ️ If status=0 (IN_PROGRESS), call resolve() first")
	t.Logf("")
	t.Logf("   Check game status:")
	t.Logf("   cast call %s \"status()(uint8)\" --rpc-url %s", disputeGameAddr.Hex(), sepoliaRPC)
	t.Logf("   0=IN_PROGRESS, 1=CHALLENGER_WINS, 2=DEFENDER_WINS")
	t.Logf("")
	t.Logf("   If IN_PROGRESS, run TestChallenger_Resolve_Sepolia_FullFlow first!")

	claimIndex := uint64(0)
	numToResolve := uint64(512)

	t.Logf("ResolveClaim parameters:")
	t.Logf("   - claimIndex: %d (root claim)", claimIndex)
	t.Logf("   - numToResolve: %d", numToResolve)

	txData := common.Hex2Bytes("03c2924d")
	txData = append(txData, common.LeftPadBytes(big.NewInt(int64(claimIndex)).Bytes(), 32)...)
	txData = append(txData, common.LeftPadBytes(big.NewInt(int64(numToResolve)).Bytes(), 32)...)
	t.Logf("Packed transaction data, length: %d bytes", len(txData))

	dynamicTx := &types.DynamicFeeTx{
		ChainID:   chainID,
		Nonce:     currentNonce,
		GasTipCap: new(big.Int).Mul(gasPrice, big.NewInt(2)),
		GasFeeCap: new(big.Int).Mul(gasPrice, big.NewInt(3)),
		Gas:       1000000,
		To:        &disputeGameAddr,
		Value:     big.NewInt(0),
		Data:      txData,
	}

	tx := types.NewTx(dynamicTx)

	t.Logf("Created EIP-1559 ResolveClaim Transaction:")
	t.Logf("   - Type: %d (EIP-1559)", tx.Type())
	t.Logf("   - ChainID: %s", chainID)
	t.Logf("   - Nonce: %d", tx.Nonce())
	t.Logf("   - To: %s (DisputeGame)", disputeGameAddr.Hex())
	t.Logf("   - Gas: %d", tx.Gas())
	t.Logf("   - Data length: %d bytes", len(tx.Data()))

	xlayerConfig := XLayerConfig{
		Endpoint:        "http://asset-onchain.base-global.svc.test.local:7001",
		Address:         challengerAddr.Hex(),
		UserID:          0,
		Symbol:          2882,
		ProjectSymbol:   3011,
		OperateSymbol:   2,
		OperateAmount:   "0",
		SysFrom:         3,
		RequestSignURI:  "/priapi/v1/assetonchain/ecology/ecologyOperate",
		QuerySignURI:    "/priapi/v1/assetonchain/ecology/querySignDataByOrderNo",
		DepositeAddress: "",
		AccessKey:       "test-access-key",
		SecretKey:       "test-secret-key",
	}

	xlayerClient := NewXLayerRemoteClient(logger, xlayerConfig)

	t.Logf(" Sending remote signing request to XLayer (operateType=21 for resolveClaim)...")
	signCtx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	signedTx, err := xlayerClient.SignTransaction(signCtx, chainID, challengerAddr, tx)
	if err != nil {
		t.Fatalf(" Remote signing failed: %v", err)
	}

	require.NotNil(t, signedTx, "Signed transaction should not be nil")
	t.Logf(" Successfully signed transaction: %s", signedTx.Hash().Hex())

	t.Logf(" Signed Transaction Details:")
	t.Logf("   - Type: %d", signedTx.Type())
	t.Logf("   - ChainID: %s", signedTx.ChainId())
	t.Logf("   - Nonce: %d", signedTx.Nonce())
	t.Logf("   - Gas: %d", signedTx.Gas())
	t.Logf("   - Value: %s wei", signedTx.Value())

	signer := types.LatestSignerForChainID(signedTx.ChainId())
	recoveredFrom, err := signer.Sender(signedTx)
	require.NoError(t, err, "Failed to recover sender")

	t.Logf(" Signature Verification:")
	t.Logf("   - Expected Sender: %s", challengerAddr.Hex())
	t.Logf("   - Recovered Sender: %s", recoveredFrom.Hex())

	t.Logf(" Sending resolveClaim transaction to Sepolia testnet...")
	sendCtx, sendCancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer sendCancel()

	err = client.SendTransaction(sendCtx, signedTx)
	if err != nil {
		t.Logf(" Failed to send transaction: %v", err)
		t.Logf("This might be due to:")
		t.Logf("   - Insufficient balance in challenger address")
		t.Logf("   - Nonce mismatch")
		t.Logf("   - Gas price too low")
		t.Logf("   - DisputeGame not in resolvable state")
		t.Logf("   - Invalid claimIndex")
	} else {
		t.Logf(" Transaction sent successfully!")
		t.Logf("   Transaction Hash: %s", signedTx.Hash().Hex())
		t.Logf("   View on Etherscan: https://sepolia.etherscan.io/tx/%s", signedTx.Hash().Hex())

		t.Logf(" Waiting for transaction confirmation...")
		receipt, err := waitForChallengerReceipt(client, signedTx.Hash(), 2*time.Minute)
		if err != nil {
			t.Logf(" Failed to get receipt: %v", err)
		} else {
			t.Logf(" Transaction confirmed!")
			t.Logf("   Receipt Details:")
			t.Logf("   - Block Number: %d", receipt.BlockNumber)
			t.Logf("   - Status: %d (1=success, 0=failed)", receipt.Status)
			t.Logf("   - Gas Used: %d", receipt.GasUsed)

			if receipt.Status == types.ReceiptStatusSuccessful {
				t.Logf(" ResolveClaim transaction executed successfully!")
			} else {
				t.Logf(" ResolveClaim transaction reverted")
				t.Logf("   Possible reasons:")
				t.Logf("   1. Game not in CHALLENGER_WINS status")
				t.Logf("   2. ClaimIndex doesn't exist")
				t.Logf("   3. Claim already resolved")
				t.Logf("   4. Chess clock not expired")
				t.Logf("   5. Not enough gas")
				t.Logf("   6. Game hasn't reached resolution time")
				t.Logf("")
				t.Logf("   cast call %s \"status()(uint8)\" --rpc-url <RPC>", disputeGameAddr.Hex())
				t.Logf("   cast call %s \"resolvedAt()(uint64)\" --rpc-url <RPC>", disputeGameAddr.Hex())
				t.Logf("   cast call %s \"claimData(uint256)\" %d --rpc-url <RPC>", disputeGameAddr.Hex(), claimIndex)
			}

			t.Logf("   View on Etherscan: https://sepolia.etherscan.io/tx/%s", receipt.TxHash.Hex())
		}
	}

	t.Logf(" Full ResolveClaim flow test completed")
}

func TestChallenger_Resolve_Sepolia_FullFlow(t *testing.T) {
	// t.Skip("Skipping Sepolia on-chain test. Remove this line to run manually.")

	logger := testlog.Logger(t, log.LevelInfo)

	sepoliaRPC := "https://www.okx.com/fullnode/xlayer/ethsepolia/discover/rpc"
	client, err := ethclient.Dial(sepoliaRPC)
	require.NoError(t, err, "Failed to connect to Sepolia")
	defer client.Close()

	t.Logf(" Connected to Sepolia testnet")

	chainID := big.NewInt(11155111)
	challengerAddr := common.HexToAddress("0x1a13bddcc02d363366e04d4aa588d3c125b0ff6f")
	disputeGameAddr := common.HexToAddress("0x336641134aeed9fa85e2caa145bfc3fbc234fd08")

	ctx := context.Background()
	currentNonce, err := client.PendingNonceAt(ctx, challengerAddr)
	require.NoError(t, err, "Failed to get nonce")
	t.Logf("Current nonce for %s: %d", challengerAddr.Hex(), currentNonce)

	gasPrice, err := client.SuggestGasPrice(ctx)
	require.NoError(t, err, "Failed to get gas price")
	t.Logf("Suggested gas price: %s wei", gasPrice)

	t.Logf("Resolve parameters: (no parameters)")

	txData := common.Hex2Bytes("2810e1d6")
	t.Logf("Packed transaction data, length: %d bytes", len(txData))

	dynamicTx := &types.DynamicFeeTx{
		ChainID:   chainID,
		Nonce:     currentNonce,
		GasTipCap: new(big.Int).Mul(gasPrice, big.NewInt(2)),
		GasFeeCap: new(big.Int).Mul(gasPrice, big.NewInt(3)),
		Gas:       1000000,
		To:        &disputeGameAddr,
		Value:     big.NewInt(0),
		Data:      txData,
	}

	tx := types.NewTx(dynamicTx)

	t.Logf("Created EIP-1559 Resolve Transaction:")
	t.Logf("   - Type: %d (EIP-1559)", tx.Type())
	t.Logf("   - To: %s (DisputeGame)", disputeGameAddr.Hex())
	t.Logf("   - Gas: %d", tx.Gas())

	xlayerConfig := XLayerConfig{
		Endpoint:        "http://asset-onchain.base-global.svc.test.local:7001",
		Address:         challengerAddr.Hex(),
		UserID:          0,
		Symbol:          2882,
		ProjectSymbol:   3011,
		OperateSymbol:   2,
		OperateAmount:   "0",
		SysFrom:         3,
		RequestSignURI:  "/priapi/v1/assetonchain/ecology/ecologyOperate",
		QuerySignURI:    "/priapi/v1/assetonchain/ecology/querySignDataByOrderNo",
		DepositeAddress: "",
		AccessKey:       "test-access-key",
		SecretKey:       "test-secret-key",
	}

	xlayerClient := NewXLayerRemoteClient(logger, xlayerConfig)

	t.Logf(" Sending remote signing request (operateType=22 for resolve)...")
	signCtx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	signedTx, err := xlayerClient.SignTransaction(signCtx, chainID, challengerAddr, tx)
	if err != nil {
		t.Fatalf(" Remote signing failed: %v", err)

	}

	require.NotNil(t, signedTx, "Signed transaction should not be nil")
	t.Logf(" Successfully signed transaction: %s", signedTx.Hash().Hex())

	t.Logf(" Sending resolve transaction to Sepolia...")
	sendCtx, sendCancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer sendCancel()

	err = client.SendTransaction(sendCtx, signedTx)
	if err != nil {
		t.Logf(" Failed to send transaction: %v", err)
	} else {
		t.Logf(" Transaction sent successfully!")
		t.Logf("   View on Etherscan: https://sepolia.etherscan.io/tx/%s", signedTx.Hash().Hex())

		receipt, err := waitForChallengerReceipt(client, signedTx.Hash(), 2*time.Minute)
		if err != nil {
			t.Logf(" Failed to get receipt: %v", err)
		} else {
			t.Logf(" Transaction confirmed! Status: %d", receipt.Status)
		}
	}

	t.Logf(" Full Resolve flow test completed")
}

func TestChallenger_ClaimCredit_Sepolia_FullFlow(t *testing.T) {
	// t.Skip("Skipping Sepolia on-chain test. Remove this line to run manually.")

	logger := testlog.Logger(t, log.LevelInfo)

	sepoliaRPC := "https://www.okx.com/fullnode/xlayer/ethsepolia/discover/rpc"
	client, err := ethclient.Dial(sepoliaRPC)
	require.NoError(t, err, "Failed to connect to Sepolia")
	defer client.Close()

	t.Logf(" Connected to Sepolia testnet")

	chainID := big.NewInt(11155111)
	challengerAddr := common.HexToAddress("0x1a13bddcc02d363366e04d4aa588d3c125b0ff6f")
	disputeGameAddr := common.HexToAddress("0x336641134aeed9fa85e2caa145bfc3fbc234fd08")
	recipient := common.HexToAddress("0x642229f238fb9dE03374Be34B0eD8D9De80752c5")

	ctx := context.Background()
	currentNonce, err := client.PendingNonceAt(ctx, challengerAddr)
	require.NoError(t, err, "Failed to get nonce")
	t.Logf("Current nonce for %s: %d", challengerAddr.Hex(), currentNonce)

	gasPrice, err := client.SuggestGasPrice(ctx)
	require.NoError(t, err, "Failed to get gas price")
	t.Logf("Suggested gas price: %s wei", gasPrice)

	t.Logf("ClaimCredit parameters:")
	t.Logf("   - recipient: %s", recipient.Hex())

	txData := common.Hex2Bytes("60e27464")
	txData = append(txData, common.LeftPadBytes(recipient.Bytes(), 32)...)
	t.Logf("Packed transaction data, length: %d bytes", len(txData))

	dynamicTx := &types.DynamicFeeTx{
		ChainID:   chainID,
		Nonce:     currentNonce,
		GasTipCap: new(big.Int).Mul(gasPrice, big.NewInt(2)),
		GasFeeCap: new(big.Int).Mul(gasPrice, big.NewInt(3)),
		Gas:       1000000,
		To:        &disputeGameAddr,
		Value:     big.NewInt(0),
		Data:      txData,
	}

	tx := types.NewTx(dynamicTx)

	xlayerConfig := XLayerConfig{
		Endpoint:        "http://asset-onchain.base-global.svc.test.local:7001",
		Address:         challengerAddr.Hex(),
		UserID:          0,
		Symbol:          2882,
		ProjectSymbol:   3011,
		OperateSymbol:   2,
		OperateAmount:   "0",
		SysFrom:         3,
		RequestSignURI:  "/priapi/v1/assetonchain/ecology/ecologyOperate",
		QuerySignURI:    "/priapi/v1/assetonchain/ecology/querySignDataByOrderNo",
		DepositeAddress: "",
		AccessKey:       "test-access-key",
		SecretKey:       "test-secret-key",
	}

	xlayerClient := NewXLayerRemoteClient(logger, xlayerConfig)

	t.Logf(" Sending remote signing request (operateType=23 for claimCredit)...")
	signCtx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	signedTx, err := xlayerClient.SignTransaction(signCtx, chainID, challengerAddr, tx)
	if err != nil {

		t.Fatalf(" Remote signing failed: %v", err)

	}

	require.NotNil(t, signedTx, "Signed transaction should not be nil")
	t.Logf(" Successfully signed transaction: %s", signedTx.Hash().Hex())

	t.Logf(" Sending claimCredit transaction to Sepolia...")
	sendCtx, sendCancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer sendCancel()

	err = client.SendTransaction(sendCtx, signedTx)
	if err != nil {
		t.Logf(" Failed to send transaction: %v", err)
	} else {
		t.Logf(" Transaction sent successfully!")
		t.Logf("  View on Etherscan: https://sepolia.etherscan.io/tx/%s", signedTx.Hash().Hex())

		receipt, err := waitForChallengerReceipt(client, signedTx.Hash(), 2*time.Minute)
		if err != nil {
			t.Logf(" Failed to get receipt: %v", err)
		} else {
			t.Logf(" Transaction confirmed! Status: %d", receipt.Status)
		}
	}

	t.Logf(" Full ClaimCredit flow test completed")
}

func waitForChallengerReceipt(client *ethclient.Client, txHash common.Hash, timeout time.Duration) (*types.Receipt, error) {
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
