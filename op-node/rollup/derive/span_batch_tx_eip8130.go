package derive

import (
	"bytes"
	"fmt"
	"io"
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/rlp"
)

// xlayerAATxType is the EIP-2718 type byte for XLayerAA (EIP-8130) transactions.
const xlayerAATxType = byte(0x7B)

// spanBatchAATxData is the span-batch-compressed form of a 0x7B AA transaction.
//
// Fields NOT stored here (kept in span-batch headers):
//   - chain_id      — implicit from rollup config
//   - nonce_sequence — in txNonces column
//   - gas_limit      — in txGases column
//
// The complex nested fields (account_changes, calls) are kept as raw RLP bytes
// so we don't need a full Go type hierarchy — they round-trip opaquely and are
// wire-compatible with the Rust kona SpanBatchEip8130TransactionData.
type spanBatchAATxData struct {
	From              common.Address // zero ⇒ EOA recovery mode
	NonceKey          *big.Int       // 2D-nonce channel selector
	Expiry            uint64
	GasPrice          *big.Int
	AccountChangesRaw rlp.RawValue // raw RLP of account_changes list
	CallsRaw          rlp.RawValue // raw RLP of calls list-of-lists
	Payer             common.Address // zero ⇒ self-pay
	SenderAuth        []byte
	PayerAuth         []byte
}

func (d *spanBatchAATxData) txType() byte { return xlayerAATxType }

// aaSpanBody mirrors the wire layout of SpanBatchEip8130TransactionData (Rust).
// [20]byte fields encode as 20-byte RLP strings (zero address = None).
type aaSpanBody struct {
	From           [20]byte
	NonceKey       *big.Int
	Expiry         uint64
	GasPrice       *big.Int
	AccountChanges rlp.RawValue
	Calls          rlp.RawValue
	Payer          [20]byte
	SenderAuth     []byte
	PayerAuth      []byte
}

// EncodeRLP writes the span-batch body (RLP list) after the 0x7B type byte.
func (d *spanBatchAATxData) EncodeRLP(w io.Writer) error {
	return rlp.Encode(w, aaSpanBody{
		From:           [20]byte(d.From),
		NonceKey:       d.NonceKey,
		Expiry:         d.Expiry,
		GasPrice:       d.GasPrice,
		AccountChanges: d.AccountChangesRaw,
		Calls:          d.CallsRaw,
		Payer:          [20]byte(d.Payer),
		SenderAuth:     d.SenderAuth,
		PayerAuth:      d.PayerAuth,
	})
}

// DecodeRLP reads the span-batch body (RLP list) from s.
func (d *spanBatchAATxData) DecodeRLP(s *rlp.Stream) error {
	var body aaSpanBody
	if err := s.Decode(&body); err != nil {
		return fmt.Errorf("failed to decode spanBatchAATxData: %w", err)
	}
	d.From = common.Address(body.From)
	d.NonceKey = body.NonceKey
	if d.NonceKey == nil {
		d.NonceKey = new(big.Int)
	}
	d.Expiry = body.Expiry
	d.GasPrice = body.GasPrice
	if d.GasPrice == nil {
		d.GasPrice = new(big.Int)
	}
	d.AccountChangesRaw = body.AccountChanges
	d.CallsRaw = body.Calls
	d.Payer = common.Address(body.Payer)
	d.SenderAuth = body.SenderAuth
	d.PayerAuth = body.PayerAuth
	return nil
}

// aaOptionalAddress encodes an address as an RLP optional:
// zero address → 0x80 (empty string), non-zero → 20-byte string.
// Matches Rust's encode_optional_address / decode_optional_address.
type aaOptionalAddress [20]byte

func (a aaOptionalAddress) EncodeRLP(w io.Writer) error {
	if a == (aaOptionalAddress{}) {
		_, err := w.Write([]byte{0x80})
		return err
	}
	return rlp.Encode(w, []byte(a[:]))
}

// aaFullTx is a helper struct for constructing the full 2718 AA tx RLP body.
type aaFullTx struct {
	ChainID        uint64
	From           aaOptionalAddress
	NonceKey       *big.Int
	NonceSequence  uint64
	Expiry         uint64
	GasPrice       *big.Int
	GasLimit       uint64
	AccountChanges rlp.RawValue
	Calls          rlp.RawValue
	Payer          aaOptionalAddress
	SenderAuth     []byte
	PayerAuth      []byte
}

// toRawTx reconstructs the full EIP-2718 encoded AA tx from span-batch data.
func (d *spanBatchAATxData) toRawTx(nonce, gas, chainID uint64) ([]byte, error) {
	inner, err := rlp.EncodeToBytes(aaFullTx{
		ChainID:        chainID,
		From:           aaOptionalAddress(d.From),
		NonceKey:       d.NonceKey,
		NonceSequence:  nonce,
		Expiry:         d.Expiry,
		GasPrice:       d.GasPrice,
		GasLimit:       gas,
		AccountChanges: d.AccountChangesRaw,
		Calls:          d.CallsRaw,
		Payer:          aaOptionalAddress(d.Payer),
		SenderAuth:     d.SenderAuth,
		PayerAuth:      d.PayerAuth,
	})
	if err != nil {
		return nil, fmt.Errorf("failed to encode AA full tx: %w", err)
	}
	return append([]byte{xlayerAATxType}, inner...), nil
}

// decodeAATxToSpanBatchData parses a raw EIP-2718 AA tx and extracts the
// span-batch-compressed fields.  Returns (data, chainID, nonceSeq, gasLimit).
func decodeAATxToSpanBatchData(rawTx []byte) (*spanBatchAATxData, uint64, uint64, uint64, error) {
	if len(rawTx) < 2 || rawTx[0] != xlayerAATxType {
		return nil, 0, 0, 0, fmt.Errorf("not an AA tx (type 0x%02x)", rawTx[0])
	}

	s := rlp.NewStream(bytes.NewReader(rawTx[1:]), 0)

	_, err := s.List()
	if err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to read list header: %w", err)
	}

	var chainID uint64
	if err := s.Decode(&chainID); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode chain_id: %w", err)
	}

	from, err := decodeOptionalAddr(s)
	if err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode from: %w", err)
	}

	var nonceKey *big.Int
	if err := s.Decode(&nonceKey); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode nonce_key: %w", err)
	}
	if nonceKey == nil {
		nonceKey = new(big.Int)
	}

	var nonceSeq uint64
	if err := s.Decode(&nonceSeq); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode nonce_sequence: %w", err)
	}

	var expiry uint64
	if err := s.Decode(&expiry); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode expiry: %w", err)
	}

	var gasPrice *big.Int
	if err := s.Decode(&gasPrice); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode gas_price: %w", err)
	}
	if gasPrice == nil {
		gasPrice = new(big.Int)
	}

	var gasLimit uint64
	if err := s.Decode(&gasLimit); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode gas_limit: %w", err)
	}

	var accountChanges rlp.RawValue
	if err := s.Decode(&accountChanges); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode account_changes: %w", err)
	}

	var calls rlp.RawValue
	if err := s.Decode(&calls); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode calls: %w", err)
	}

	payer, err := decodeOptionalAddr(s)
	if err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode payer: %w", err)
	}

	var senderAuth []byte
	if err := s.Decode(&senderAuth); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode sender_auth: %w", err)
	}

	var payerAuth []byte
	if err := s.Decode(&payerAuth); err != nil {
		return nil, 0, 0, 0, fmt.Errorf("AA tx: failed to decode payer_auth: %w", err)
	}

	data := &spanBatchAATxData{
		From:              from,
		NonceKey:          nonceKey,
		Expiry:            expiry,
		GasPrice:          gasPrice,
		AccountChangesRaw: accountChanges,
		CallsRaw:          calls,
		Payer:             payer,
		SenderAuth:        senderAuth,
		PayerAuth:         payerAuth,
	}
	return data, chainID, nonceSeq, gasLimit, nil
}

// decodeOptionalAddr reads an RLP bytes field and interprets it as an optional
// address: empty bytes → zero address (None), 20 bytes → address (Some).
func decodeOptionalAddr(s *rlp.Stream) (common.Address, error) {
	b, err := s.Bytes()
	if err != nil {
		return common.Address{}, err
	}
	switch len(b) {
	case 0:
		return common.Address{}, nil
	case 20:
		return common.BytesToAddress(b), nil
	default:
		return common.Address{}, fmt.Errorf("invalid optional address length: %d", len(b))
	}
}
