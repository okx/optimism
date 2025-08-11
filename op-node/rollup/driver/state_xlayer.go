package driver

import (
	"context"

	"github.com/ethereum-optimism/optimism/op-node/rollup/sequencing"
)

func (s *Driver) SetMaxSafeLag(ctx context.Context, v uint64) error {
	return s.sequencer.SetMaxSafeLag(ctx, v)
}

func (s *Driver) SetSequencerConfDepth(ctx context.Context, depth uint64) error {
	// Update the driver config
	s.driverConfig.SequencerConfDepth = depth

	// If sequencer is not enabled, no need to update anything else
	if !s.driverConfig.SequencerEnabled {
		return nil
	}

	// Try to update the actual confDepth instance through sequencer
	if seq, ok := s.sequencer.(*sequencing.Sequencer); ok {
		if err := seq.SetConfDepth(depth); err != nil {
			s.log.Warn("Failed to update sequencer confirmation depth", "error", err)
		} else {
			s.log.Info("Successfully updated sequencer confirmation depth", "new_depth", depth)
		}
	} else {
		s.log.Info("Updated sequencer confirmation depth in config", "new_depth", depth)
	}

	return nil
}

func (s *Driver) SetVerifierConfDepth(ctx context.Context, depth uint64) error {
	// Update the driver config
	s.driverConfig.VerifierConfDepth = depth

	// For verifier conf depth, we need to access the derivation pipeline's verifConfDepth
	// Since this affects the derivation pipeline, we log the update
	// The actual confDepth instance is used by the derivation pipeline
	s.log.Info("Updated verifier confirmation depth in config", "new_depth", depth)

	// Note: Unlike sequencer conf depth, verifier conf depth affects the derivation pipeline
	// which uses the verifConfDepth instance created during driver initialization.
	// Dynamic updates to this would require pipeline reconstruction or a similar mechanism
	// to the sequencer approach, but would need access to the pipeline's verifConfDepth instance.

	return nil
}
