package signer

import (
	"bytes"
	"crypto/aes"
	"crypto/cipher"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"
)

const (
	headerSignKey   = "sign"
	headerAccessKey = "accessKey"
	algoSha256      = "sha256"
)

// addAuth adds authentication headers to the request
// Uses the same authentication mechanism as xlayer-node: accessKey + SHA256 signature + AES encryption
// If SecretKey or AccessKey is empty, no authentication is performed
func (c *XLayerRemoteClient) addAuth(req *http.Request) error {
	if c.config.SecretKey == "" || c.config.AccessKey == "" {
		return nil
	}

	// Set accessKey header
	req.Header.Set(headerAccessKey, c.config.AccessKey)

	// Generate signature using SHA256 algorithm
	signature, err := c.genAuth(req, algoSha256)
	if err != nil {
		return fmt.Errorf("failed to generate signature: %w", err)
	}

	// Set sign header
	req.Header.Set(headerSignKey, signature)

	return nil
}

// genAuth generates authentication signature from HTTP request
func (c *XLayerRemoteClient) genAuth(req *http.Request, algorithm string) (string, error) {
	// 1. Process URL query parameters
	params := req.URL.Query()
	treeMap := make(map[string][]string)
	for _, v := range params {
		var key string
		for _, vv := range v {
			key += vv
		}
		treeMap[key] = v
	}

	// 2. Read request body
	var body string
	if req.Body != nil {
		readCloser, err := req.GetBody()
		if err != nil {
			return "", fmt.Errorf("get body error: %w", err)
		}
		defer readCloser.Close()

		buffer, err := io.ReadAll(readCloser)
		if err != nil {
			return "", fmt.Errorf("read body error: %w", err)
		}

		if len(buffer) > 0 {
			body = string(buffer)
		}
	}

	// 3. Generate signature from treeMap and body
	return c.generateSignature(treeMap, body, algorithm)
}

// generateSignature generates signature from parameters and body
func (c *XLayerRemoteClient) generateSignature(treeMap map[string][]string, body, algorithm string) (string, error) {
	// Sort the map by values
	var keys []string
	for key := range treeMap {
		keys = append(keys, key)
	}
	sort.Strings(keys)

	// Construct the content string
	var content strings.Builder
	for _, key := range keys {
		values := treeMap[key]
		for _, v := range values {
			content.WriteString(v)
		}
	}

	// Append the body if present
	if body != "" {
		content.WriteString(body)
	}

	// Calculate the hash based on the selected algorithm
	var hash []byte
	switch strings.ToLower(algorithm) {
	case algoSha256:
		hashObj := sha256.New()
		hashObj.Write([]byte(content.String()))
		hash = hashObj.Sum(nil)
	default:
		return "", fmt.Errorf("unsupported algorithm: %s", algorithm)
	}

	// Convert the hash to a hexadecimal string
	hashString := hex.EncodeToString(hash)

	// Encrypt the hash using AES
	return encryptAES(hashString, c.config.SecretKey)
}

// encryptAES encrypts using AES ECB mode (consistent with xlayer-node)
func encryptAES(src, key string) (string, error) {
	block, err := aes.NewCipher([]byte(key))
	if err != nil {
		return "", fmt.Errorf("failed to create AES cipher: %w", err)
	}

	ecbEncrypt := newECBEncrypter(block)
	content := []byte(src)
	content = pkcs5Padding(content, block.BlockSize())
	encrypted := make([]byte, len(content))

	err = ecbEncrypt.cryptBlocksWithError(encrypted, content)
	if err != nil {
		return "", fmt.Errorf("failed to encrypt AES: %w", err)
	}

	return base64.StdEncoding.EncodeToString(encrypted), nil
}

// pkcs5Padding applies PKCS5 padding
func pkcs5Padding(ciphertext []byte, blockSize int) []byte {
	padding := blockSize - len(ciphertext)%blockSize
	padText := bytes.Repeat([]byte{byte(padding)}, padding)
	return append(ciphertext, padText...)
}

// ecb represents Electronic Code Book mode
type ecb struct {
	b         cipher.Block
	blockSize int
}

// newECB creates a new ECB instance
func newECB(b cipher.Block) *ecb {
	return &ecb{
		b:         b,
		blockSize: b.BlockSize(),
	}
}

// ecbEncrypter implements ECB encryption
type ecbEncrypter ecb

// newECBEncrypter returns a BlockMode which encrypts in electronic code book mode
func newECBEncrypter(b cipher.Block) *ecbEncrypter {
	return (*ecbEncrypter)(newECB(b))
}

// BlockSize returns the block size
func (x *ecbEncrypter) BlockSize() int {
	return x.blockSize
}

// CryptBlocks encrypts blocks of data
func (x *ecbEncrypter) CryptBlocks(dst, src []byte) {
	if len(src)%x.blockSize != 0 {
		panic("crypto/cipher: input not full blocks")
	}
	if len(dst) < len(src) {
		panic("crypto/cipher: output smaller than input")
	}
	for len(src) > 0 {
		x.b.Encrypt(dst, src[:x.blockSize])
		src = src[x.blockSize:]
		dst = dst[x.blockSize:]
	}
}

// cryptBlocksWithError encrypts blocks with error handling instead of panic
func (x *ecbEncrypter) cryptBlocksWithError(dst, src []byte) error {
	if len(src)%x.blockSize != 0 {
		return fmt.Errorf("crypto/cipher: input not full blocks %d, %d", len(src), x.blockSize)
	}
	if len(dst) < len(src) {
		return fmt.Errorf("crypto/cipher: output smaller than input %d, %d", len(dst), len(src))
	}

	x.CryptBlocks(dst, src)

	return nil
}
