/** The Wallet tab: coins under the user's own key, outside Keel, and the move into Keel. */
import { useState } from 'react';
import type { UiState } from '../ui';
import { SelfCustody } from '../components/SelfCustody';

export function Wallet({ state }: { state: UiState }) {
  const { network, address } = state;
  const [refreshKey, setRefreshKey] = useState(0);
  if (!address || network.placeholder) return <div className="page"><p className="muted">{network.placeholder ? `${network.name}: no RPC configured yet.` : 'No account.'}</p></div>;
  if (network.id !== 'mainnet' && network.id !== 'testnet') {
    return <div className="page"><p className="muted">Your own addresses are available on the testnet and mainnet, where the wallet knows the public chain APIs.</p></div>;
  }
  return (
    <div className="page">
      <SelfCustody networkId={network.id} address={address} refreshKey={refreshKey} onMoved={() => setRefreshKey((k) => k + 1)} />
      <p className="muted small">To bring coins back here, use <b>Send → Move to your wallet</b>. New deposits can land here by default from Settings.</p>
    </div>
  );
}
