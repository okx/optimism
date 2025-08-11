package sequencing

import "fmt"

// SetConfDepth updates the confirmation depth if the underlying L1Blocks supports it
func (los *L1OriginSelector) SetConfDepth(depth uint64) error {
	// Try to cast to confDepth and update the depth
	if confDepth, ok := los.l1.(interface{ SetDepth(uint64) }); ok {
		confDepth.SetDepth(depth)
		return nil
	}
	return fmt.Errorf("L1Blocks does not support SetDepth")
}
