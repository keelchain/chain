/* Keelchain docs: the Supported networks page reads the chain it describes.
 *
 * The page is rendered from docs/networks.md. Its shaded boxes are
 * `<div class="live" data-live="…">` placeholders and the governance
 * numbers in its prose are `<span data-param="…">` with the default as
 * text. This script reads the node RPC and the read API named on its own
 * <script data-rpc data-api> tag, fills the boxes and refreshes the spans.
 * Anything that fails to load leaves the static text and says so in the
 * box; without JavaScript the page reads as written.
 */
(() => {
  const me = document.currentScript || document.querySelector('script[data-rpc]');
  const RPC = (me && me.dataset.rpc) || 'https://testnet.keelchain.com/rpc';
  const API = (me && me.dataset.api) || 'https://testnet.keelchain.com/api';
  const CHAINS = ['BTC', 'ETH', 'TRON', 'BSC', 'SOL'];
  const NAME = { BTC: 'Bitcoin', ETH: 'Ethereum', TRON: 'Tron', BSC: 'BNB Smart Chain', SOL: 'Solana' };
  // What the testnet vaults live on. Mainnet names are the chain's own.
  const EXTERNAL = { BTC: 'signet', ETH: 'Sepolia', TRON: 'Nile', BSC: 'BSC testnet (97)', SOL: 'devnet' };
  const EXPLORER = {
    BTC: 'https://mempool.space/signet',
    ETH: 'https://sepolia.etherscan.io',
    TRON: 'https://nile.tronscan.org/#',
    BSC: 'https://testnet.bscscan.com',
    SOL: 'https://solscan.io',
  };
  // Which depth parameter each chain reads (keel-vm vaults::confirmations).
  const DEPTH_PARAM = { BTC: 'confirmations_btc', ETH: 'confirmations_eth', BSC: 'confirmations_eth', TRON: 'confirmations_tron', SOL: 'confirmations_tron' };

  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
  const short = (hex, n = 8) => (hex && hex.length > 2 * n + 1 ? `${hex.slice(0, n)}…${hex.slice(-n)}` : hex || '');
  const intFmt = new Intl.NumberFormat('en-US');
  const fmtInt = (v) => (v === null || v === undefined ? '–' : intFmt.format(typeof v === 'bigint' ? v : Number(v)));

  // Amounts are integers in smallest units and may exceed 2^53: format as strings.
  function fmtAmount(raw, decimals, maxFrac = 6) {
    if (raw === null || raw === undefined) return '–';
    let s = String(raw).replace(/^-/, '');
    const neg = String(raw).startsWith('-');
    const d = Number(decimals) || 0;
    if (s.length <= d) s = '0'.repeat(d - s.length + 1) + s;
    const whole = s.slice(0, s.length - d) || '0';
    let frac = d ? s.slice(s.length - d) : '';
    frac = frac.slice(0, maxFrac).replace(/0+$/, '');
    const w = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return (neg ? '-' : '') + w + (frac ? '.' + frac : '');
  }
  const usdMicro = (v) => '$' + fmtAmount(v, 6, 2);
  const bps = (v) => `${(Number(v) / 100).toLocaleString('en-US', { maximumFractionDigits: 2 })}%`;
  const sats = (v) => `${fmtAmount(v, 8, 8)} BTC`;

  // Fee rate in the unit the observers quote per chain (keel-chains FeeRate).
  function feeRate(chain, rate) {
    if (rate === null || rate === undefined) return '–';
    switch (chain) {
      case 'BTC': return `${fmtInt(rate)} sat/vB`;
      case 'ETH':
      case 'BSC': return `${fmtAmount(rate, 9, 3)} gwei/gas`;
      case 'TRON': return `${fmtAmount(rate, 6, 2)} TRX/tx`;
      case 'SOL': return `${fmtInt(rate)} lamports/sig`;
      default: return fmtInt(rate);
    }
  }
  const ago = (secs) => {
    if (secs === null || secs === undefined) return '';
    const s = Number(secs);
    if (s < 90) return `${Math.round(s)} s ago`;
    if (s < 5400) return `${Math.round(s / 60)} min ago`;
    if (s < 172800) return `${Math.round(s / 3600)} h ago`;
    return `${Math.round(s / 86400)} d ago`;
  };

  async function getJson(url) {
    const r = await fetch(url, { headers: { accept: 'application/json' } });
    if (!r.ok) throw new Error(`${r.status} ${url}`);
    return r.json();
  }
  // Settles to the value or to null, never rejects: one failing call must not blank the page.
  const soft = (p) => p.then((v) => ({ ok: true, v }), (e) => ({ ok: false, e }));

  const boxes = {};
  for (const el of document.querySelectorAll('[data-live]')) boxes[el.dataset.live] = el;
  function fill(key, html, source) {
    const el = boxes[key];
    if (!el) return;
    el.innerHTML = `<p class="asof">Live from <code>${esc(source)}</code>, ${new Date().toLocaleTimeString()}.</p>${html}`;
  }
  function fail(key, source) {
    const el = boxes[key];
    if (!el) return;
    el.innerHTML = `<p class="asof err">Could not read <code>${esc(source)}</code> just now; the text on this page still applies.</p>`;
  }
  const readyMark = (r) => (r && r.ready ? '<span class="ok">ready</span>' : '<span class="bad">not ready</span>');

  // The dotted params answer lives flat at the top level and also nested
  // (`stable.rebalance_max_premium_bps` under `stable`); look in both.
  function param(params, key) {
    if (!params) return undefined;
    if (key in params) return params[key];
    return key.split('.').reduce((o, k) => (o && typeof o === 'object' ? o[k] : undefined), params);
  }
  function fillParams(params) {
    for (const el of document.querySelectorAll('[data-param]')) {
      const v = param(params, el.dataset.param);
      if (v === undefined || v === null) continue;
      switch (el.dataset.unit) {
        case 'bps': el.textContent = bps(v); break;
        case 'sats': el.textContent = sats(v); break;
        case 'sats-or-none': el.textContent = Number(v) === 0 ? 'none' : sats(v); break;
        case 'usd_micro': el.textContent = usdMicro(v); break;
        case 'flag': el.textContent = Number(v) ? 'on' : 'off'; break;
        default: el.textContent = fmtInt(v);
      }
      el.title = `${el.dataset.param} = ${v}`;
    }
  }

  function overview(ready, vaults, params) {
    const byChain = {};
    for (const v of vaults || []) byChain[v.chain] = v;
    const rows = CHAINS.map((c) => {
      const r = ready[c];
      const iv = byChain[c] || {};
      const depth = param(params, DEPTH_PARAM[c]);
      const vault = r && r.vault
        ? `epoch ${r.vault.epoch}, ${r.vault.threshold} of ${r.vault.signers.length}`
        : '<span class="bad">none</span>';
      const halted = r && r.halted && r.halted.length ? `<span class="bad">${esc(r.halted.join(', '))}</span>` : 'none';
      const status = r ? readyMark(r) + (r.reasons && r.reasons.length ? `<br><small class="muted">${esc(r.reasons.join('; '))}</small>` : '') : '<span class="muted">unavailable</span>';
      return `<tr>
        <td><a href="#${NAME[c].toLowerCase().replace(/ /g, '-')}">${NAME[c]}</a><br><small class="muted">${esc(EXTERNAL[c])}</small></td>
        <td>${status}</td>
        <td>${vault}</td>
        <td class="num">${depth === undefined ? '–' : fmtInt(depth)}${c === 'SOL' ? ' slots' : ''}</td>
        <td class="num">${r ? feeRate(c, r.fee_rate) : '–'}</td>
        <td class="num">${fmtInt(iv.address_count)}</td>
        <td class="num">${fmtInt(iv.deposits)}</td>
        <td class="num">${r ? fmtInt(r.outbound_pending) : '–'}</td>
        <td>${halted}</td>
      </tr>`;
    }).join('');
    return `<table><thead><tr><th>Network</th><th>Switch</th><th>Vault</th><th class="num">Depth</th><th class="num">Fee rate now</th><th class="num">Addresses</th><th class="num">Deposits</th><th class="num">Outbounds pending</th><th>Halted</th></tr></thead><tbody>${rows}</tbody></table>`;
  }

  function chainBox(c, r, iv) {
    if (!r) return null;
    const v = r.vault;
    const rows = [];
    rows.push(['Per-network switch', readyMark(r) + (r.reasons && r.reasons.length ? ` <small class="muted">(${esc(r.reasons.join('; '))})</small>` : '')]);
    if (v) {
      rows.push(['Vault key', `<code title="${esc(v.public_key)}">${esc(short(v.public_key, 10))}</code>, epoch ${v.epoch}, registered at block ${fmtInt(v.registered_height)}`]);
      rows.push(['Signers', `${v.threshold} of ${v.signers.length} must sign`]);
    } else {
      rows.push(['Vault', '<span class="bad">not registered</span>']);
    }
    rows.push(['Depth required', `${fmtInt(param(PARAMS, DEPTH_PARAM[c]))} ${c === 'SOL' ? 'slots (finalized)' : 'confirmations'} <code>${DEPTH_PARAM[c]}</code>`]);
    rows.push(['Fee rate now', feeRate(c, r.fee_rate)]);
    if (c === 'BTC') {
      const cp = r.checkpoint;
      rows.push(['Checkpoint', cp
        ? `header ${fmtInt(cp.checkpoint_height)}, tip seen ${fmtInt(cp.tip_height)}, ${ago(cp.age_secs)}`
        : '<span class="bad">none</span>']);
    }
    if (iv) {
      rows.push(['Deposit addresses issued', fmtInt(iv.address_count)]);
      rows.push(['Deposits credited', fmtInt(iv.deposits)]);
      const res = {};
      for (const x of iv.reserves || []) res[x.asset] = x.amount;
      const liab = iv.liabilities || [];
      if (liab.length || Object.keys(res).length) {
        const assets = Array.from(new Set([...liab.map((x) => x.asset), ...Object.keys(res)])).sort();
        const dec = (a) => (DECIMALS[a] !== undefined ? DECIMALS[a] : 0);
        rows.push(['Reserves / liabilities', assets.map((a) => {
          const l = (liab.find((x) => x.asset === a) || {}).amount;
          return `<code>${esc(a)}</code> ${fmtAmount(res[a], dec(a))} held against ${fmtAmount(l, dec(a))} owed`;
        }).join('<br>')]);
      }
    }
    rows.push(['Outbounds pending', fmtInt(r.outbound_pending)]);
    rows.push(['Halted assets', r.halted && r.halted.length ? `<span class="bad">${esc(r.halted.join(', '))}</span>` : 'none']);
    if (r.last_deposit_credited !== null && r.last_deposit_credited !== undefined) {
      rows.push(['Last deposit credited', `block ${fmtInt(r.last_deposit_credited)}`]);
    }
    rows.push(['External explorer', `<a href="${EXPLORER[c]}${c === 'SOL' ? '/?cluster=devnet' : ''}" target="_blank" rel="noreferrer noopener">${esc(EXPLORER[c].replace(/^https:\/\//, '').replace(/\/#$/, ''))}</a>`]);
    return `<table><tbody>${rows.map(([k, val]) => `<tr><th>${k}</th><td>${val}</td></tr>`).join('')}</tbody></table>`;
  }

  function assetsTable(assets) {
    const kind = { native: 'native', vault: 'vault asset', stable: 'unified stable' };
    const rows = assets
      .slice()
      .sort((a, b) => (a.kind === 'native' ? -1 : b.kind === 'native' ? 1 : (a.chain || '').localeCompare(b.chain || '') || a.asset.localeCompare(b.asset)))
      .map((a) => `<tr>
        <td><code>${esc(a.asset)}</code></td>
        <td>${a.chain ? NAME[a.chain] || esc(a.chain) : 'Keel'}</td>
        <td>${kind[a.kind] || esc(a.kind)}</td>
        <td class="num">${fmtInt(a.decimals)}</td>
        <td class="num">${fmtAmount(a.supply, a.decimals, 2)}</td>
        <td class="num">${fmtInt(a.holders)}</td>
      </tr>`).join('');
    return `<table><thead><tr><th>Asset</th><th>Network</th><th>Kind</th><th class="num">Decimals</th><th class="num">In Keel</th><th class="num">Holders</th></tr></thead><tbody>${rows}</tbody></table>`;
  }

  function stableTable(st) {
    const rows = (st.basket || []).map((b) => `<tr>
      <td>${NAME[b.chain] || esc(b.chain)}<br><small class="muted"><code>${esc(b.asset)}</code>, ${b.decimals} decimals</small></td>
      <td>${b.enabled ? '<span class="ok">on</span>' : '<span class="bad">off</span>'}</td>
      <td class="num">${fmtAmount(b.reserve, b.decimals, 2)}</td>
      <td class="num">${(Number(b.share_ppm) / 10000).toFixed(1)}%</td>
      <td class="num">${(Number(b.target_ppm) / 10000).toFixed(1)}%</td>
      <td class="num">${bps(b.premium_bps)}</td>
    </tr>`).join('');
    const head = `<p><code>USDT</code> supply ${fmtAmount(st.supply, 6, 2)}, reserves ${fmtAmount(st.reserves_in_stable, 6, 2)}, rebalance pool ${fmtAmount(st.rebalance_pool, 6, 2)}; auto-convert ${Number(st.params && st.params.auto_convert) ? 'on' : 'off'}.</p>`;
    return head + `<table><thead><tr><th>Network</th><th>Enabled</th><th class="num">Reserve</th><th class="num">Share</th><th class="num">Target</th><th class="num">Premium out / rebate in</th></tr></thead><tbody>${rows}</tbody></table>`;
  }

  function lightningBox(ln) {
    const pools = ln.pools || [];
    const parts = [`<p>Lightning is <strong>${ln.enabled ? 'on' : 'off'}</strong>; ${pools.length ? `${pools.length} registered pool${pools.length === 1 ? '' : 's'}, ${sats(ln.pool_total)} lent in total` : 'no observer has registered a node yet, so no invoice can be issued or paid'}; ${(ln.assignments || []).length} open payout${(ln.assignments || []).length === 1 ? '' : 's'}, ${(ln.sweeps || []).length} pending sweep${(ln.sweeps || []).length === 1 ? '' : 's'}.</p>`];
    if (pools.length) {
      parts.push(`<table><thead><tr><th>Observer</th><th>Node</th><th class="num">On the books</th><th class="num">Pending out</th><th class="num">Available</th><th class="num">Credited today</th></tr></thead><tbody>${pools.map((p) => `<tr>
        <td><code title="${esc(p.observer || '')}">${esc(short(p.observer || '', 8))}</code></td>
        <td><code title="${esc(p.node_id || '')}">${esc(short(p.node_id || '', 8))}</code></td>
        <td class="num">${sats(p.balance)}</td>
        <td class="num">${sats(p.pending_out)}</td>
        <td class="num">${sats(p.available)}</td>
        <td class="num">${sats(p.credited_today)}</td>
      </tr>`).join('')}</tbody></table>`);
    }
    return parts.join('');
  }

  let PARAMS = null;
  const DECIMALS = {};

  async function main() {
    const [params, status, stable, ln, assets, vaults, ...readies] = await Promise.all([
      soft(getJson(`${RPC}/v1/params`)),
      soft(getJson(`${RPC}/v1/status`)),
      soft(getJson(`${RPC}/v1/stable`)),
      soft(getJson(`${RPC}/v1/lightning`)),
      soft(getJson(`${API}/v1/assets`)),
      soft(getJson(`${API}/v1/vaults`)),
      ...CHAINS.map((c) => soft(getJson(`${RPC}/v1/ready/${c}`))),
    ]);
    if (params.ok) { PARAMS = params.v; fillParams(PARAMS); }
    if (assets.ok) for (const a of assets.v) DECIMALS[a.asset] = a.decimals;
    if (stable.ok) for (const b of stable.v.basket || []) DECIMALS[b.asset] = b.decimals;

    const ready = {};
    CHAINS.forEach((c, i) => { if (readies[i].ok) ready[c] = readies[i].v; });
    const anyReady = Object.keys(ready).length > 0;
    const height = status.ok ? ` at block ${fmtInt(status.v.height)}` : '';

    if (anyReady) fill('overview', overview(ready, vaults.ok ? vaults.v : [], PARAMS), `${RPC}/v1/ready/{chain}${height}`);
    else fail('overview', `${RPC}/v1/ready/{chain}`);

    const byChain = {};
    if (vaults.ok) for (const v of vaults.v) byChain[v.chain] = v;
    for (const c of CHAINS) {
      const html = chainBox(c, ready[c], byChain[c]);
      if (html) fill(`chain:${c}`, html, `${RPC}/v1/ready/${c}${height}`);
      else fail(`chain:${c}`, `${RPC}/v1/ready/${c}`);
    }
    if (assets.ok) fill('assets', assetsTable(assets.v), `${API}/v1/assets`); else fail('assets', `${API}/v1/assets`);
    if (stable.ok) fill('stable', stableTable(stable.v), `${RPC}/v1/stable`); else fail('stable', `${RPC}/v1/stable`);
    if (ln.ok) fill('lightning', lightningBox(ln.v), `${RPC}/v1/lightning`); else fail('lightning', `${RPC}/v1/lightning`);
  }

  main().catch((e) => { console.error('networks page', e); });
})();
