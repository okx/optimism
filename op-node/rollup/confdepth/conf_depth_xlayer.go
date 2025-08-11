package confdepth

// SetDepth dynamically updates the confirmation depth
func (c *confDepth) SetDepth(depth uint64) {
	c.depth = depth
}
