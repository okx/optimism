package sources

import "github.com/ethereum-optimism/optimism/op-service/client"

func (s *L1Client) Client() client.RPC {
	return s.client
}
