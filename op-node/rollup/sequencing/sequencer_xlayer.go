package sequencing

import "fmt"

// SetConfDepth updates the confirmation depth for the sequencer
func (d *Sequencer) SetConfDepth(depth uint64) error {
	if selector, ok := d.l1OriginSelector.(*L1OriginSelector); ok {
		return selector.SetConfDepth(depth)
	}
	return fmt.Errorf("l1OriginSelector does not support SetConfDepth")
}
