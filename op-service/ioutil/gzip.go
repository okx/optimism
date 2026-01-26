package ioutil

import (
	"compress/gzip"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
)

const (
	// MaxDecompressedSize limits the maximum size of decompressed data from gzip files
	// to prevent memory exhaustion attacks via specially crafted compressed files.
	MaxDecompressedSize = 10 * 1024 * 1024 * 1024 // 10GB
)

var (
	// ErrDecompressedSizeExceeded is returned when decompressed data reaches or exceeds MaxDecompressedSize.
	ErrDecompressedSizeExceeded = errors.New("decompressed data size exceeds maximum allowed size")
)

// limitedReadCloser wraps an io.ReadCloser and enforces a maximum read limit.
// It returns ErrDecompressedSizeExceeded when the limit is exceeded.
type limitedReadCloser struct {
	limitReader *io.LimitedReader
	readCloser  io.ReadCloser
	closer      io.Closer
}

func (l *limitedReadCloser) Read(p []byte) (n int, err error) {
	n, err = l.limitReader.Read(p)

	// When EOF is reached and the read limit is exhausted, the decompressed size
	// has reached MaxDecompressedSize, which is treated as an error.
	if err == io.EOF && l.limitReader.N == 0 {
		return n, ErrDecompressedSizeExceeded
	}

	return n, err
}

func (l *limitedReadCloser) Close() error {
	return errors.Join(l.readCloser.Close(), l.closer.Close())
}

// OpenDecompressed opens a reader for the specified file and automatically decompresses gzip content
// if the filename ends with .gz. For gzip files, the decompressed output is limited to MaxDecompressedSize.
// Returns ErrDecompressedSizeExceeded if the decompressed size reaches or exceeds the limit.
func OpenDecompressed(path string) (io.ReadCloser, error) {
	r, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	if IsGzip(path) {
		gr, err := gzip.NewReader(r)
		if err != nil {
			r.Close()
			return nil, fmt.Errorf("failed to create gzip reader: %w", err)
		}
		limitReader := &io.LimitedReader{R: gr, N: MaxDecompressedSize}
		return &limitedReadCloser{
			limitReader: limitReader,
			readCloser:  gr,
			closer:      r,
		}, nil
	}
	return r, nil
}

// OpenCompressed opens a file for writing and automatically compresses the content if the filename ends with .gz
func OpenCompressed(file string, flag int, perm os.FileMode) (io.WriteCloser, error) {
	out, err := os.OpenFile(file, flag, perm)
	if err != nil {
		return nil, err
	}
	return CompressByFileType(file, out), nil
}

// WriteCompressedBytes writes a byte slice to the specified file.
// If the filename ends with .gz, a byte slice is compressed and written.
func WriteCompressedBytes(file string, data []byte, flag int, perm os.FileMode) error {
	out, err := OpenCompressed(file, flag, perm)
	if err != nil {
		return err
	}
	defer out.Close()
	_, err = out.Write(data)
	return err
}

// WriteCompressedJson writes the object to the specified file as a compressed json object
// if the filename ends with .gz.
func WriteCompressedJson(file string, obj any) error {
	if !IsGzip(file) {
		return fmt.Errorf("file %v does not have .gz extension", file)
	}
	out, err := OpenCompressed(file, os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		return err
	}
	defer out.Close()
	return json.NewEncoder(out).Encode(obj)
}

// IsGzip determines if a path points to a gzip compressed file.
// Returns true when the file has a .gz extension.
func IsGzip(path string) bool {
	return strings.HasSuffix(path, ".gz")
}

func CompressByFileType(file string, out io.WriteCloser) io.WriteCloser {
	if IsGzip(file) {
		return NewWrappedWriteCloser(gzip.NewWriter(out), out)
	}
	return out
}
