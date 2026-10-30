# Security plan: edge, hosts, replicas and switches

This is the plan for taking the testnet from one box behind proxied DNS to a
setup I would be comfortable running mainnet-shaped traffic on: a zero-trust
edge, a WAF and flood limits at three layers, a second box that keeps serving
when the first is gone, and written-down switches for incidents. It also
answers two design questions I kept coming back to: whether the chain needs
a database replica (Patroni, etcd), and what "leader election" means for the
serving layer. Short answers: no, and none is needed. The chain replicates
itself; the edge fails over by itself.

Nothing here names hosts or addresses. The topology is in
`infra/testnet/README.md`; the hosts are in the GitHub environment.

## Contents

1. Where things stand
2. How a blockchain secures itself, and what that buys here
3. Replicas: why not Patroni and etcd
4. Topology: two boxes now, four later
5. Keys on the boxes: what is threshold and what is not
6. Zero-trust edge: Cloudflare Tunnel and Access
7. WAF, rate limits and flood control at three layers
8. Host hardening
9. Switches: what I can flip, and who can flip it
10. Incident playbooks
11. Order of work

## 1. Where things stand

Already in place: Cloudflare proxied DNS with an origin certificate and
"Full (strict)"; `ufw` default-deny with SSH, the consensus port, WireGuard,
and on host 0 HTTP/HTTPS open; node RPC and indexer bound to loopback behind
nginx; per-address `limit_req` zones and an optional API-key gate on action
submission; root-only config under `/etc/keelchain`; deploys only from the
GitHub `testnet` environment over a dedicated deploy key; encrypted daily
backups; Prometheus alerts for a stalled chain, down units and a filling
disk; a WireGuard mesh for the signer protocol and Bitcoin RPC.

What is missing, found while writing this:

- **The origin is reachable directly.** Ports 22, 80, 443 and the consensus
  port answer on the public address. Cloudflare's DDoS protection only
  covers what goes through Cloudflare; anyone who knows the address goes
  around it.
- **The address is public.** Another site on the same box has an unproxied
  DNS record pointing straight at it. Until every name on the box is
  proxied or that site moves, hiding the origin is cosmetic.
- **Rate limits key on the wrong address.** nginx has no `real_ip`
  configuration, so `$binary_remote_addr` is a Cloudflare edge address, not
  the client. Everyone behind one edge address shares a bucket, and the
  faucet's per-IP limit counts edge addresses. The fix is two lines
  (`set_real_ip_from` the proxy, `real_ip_header CF-Connecting-IP`).
- **Every service runs as root.** None of the units sets `User=` or any
  sandboxing directive. A bug in the RPC server is a root shell.
- **No request body cap.** The node RPC accepts the framework default
  (2 MB) per request; nginx has no `client_max_body_size` on the API
  locations. An action is a few hundred bytes.
- **No written switches.** When something goes wrong I would be improvising
  at 3 a.m. Section 9 fixes that.
- **One box.** Everything, including the only indexer database and the
  only web origin, is on it.
- **The monitoring stack's Grafana is mapped to port 3000 on loopback,
  which is the consensus port.** It needs another port before
  `KEEL_MONITORING=1` is turned on.

## 2. How a blockchain secures itself, and what that buys here

A chain does not have a primary database that needs protecting and
replicating. It has N copies of the same state, each rebuilt
deterministically from the same ordered log of actions, and a protocol that
makes the copies agree on the next entry. The security properties come from
that shape, not from any one machine:

- **Replication is the consensus protocol.** Every validator holds the full
  state and the journal. A block is final when `2f + 1` of `n = 3f + 1`
  validators sign it. There is no primary to fail over from: with four
  validators one can vanish and blocks keep coming; with one, three or two
  validators (`f = 0`) the chain pauses, safely, until the missing one
  returns. It never forks and never loses a finalised block. "Replica" in
  chain terms means "another node", nothing else.
- **Leader election is built in.** Simplex rotates the proposer every view
  and times out a silent leader; the validator set is the electorate and
  the ballot is the block signature. No etcd, no lock service, no
  coordinator. The nodes are the coordinator.
- **The network is authenticated.** The p2p layer only completes a
  handshake with keys in the validator set or the listed followers; an
  unknown key is dropped before it costs anything. Peer slots are bounded
  (`--max-peers`), and memory is reserved per slot up front so a peer storm
  cannot grow the process.
- **State is verifiable.** Blocks are hash-linked; snapshots carry a header
  that `--sync-verify` checks against the chain before trusting them; a
  node can state-sync from any peer and detect a lie. Bitcoin checkpoints
  through governance anchor the chain's history in a system I do not
  control, so rewriting the past would show.
- **Flooding is priced without gas.** Every action spends from a
  per-address budget earned by locking KEEL (section 7 of
  `how-it-works.md`). Spam costs capital, and a spammer's own addresses run
  dry first. Validators and observers post bonds; slashing is specified.
- **Custody is threshold.** The vault key exists only as `t`-of-`n` shares
  held by separate signer daemons, each encrypted under its own passphrase;
  `t - 1` compromised shares move nothing. Deposits are credited on light
  client proofs (Bitcoin, Ethereum) or an attestation quorum (Tron), never
  on one observer's word; withdrawals are checked against the
  reserve invariant per vault, with an automatic halt when it fails.
- **Governance is slow on purpose.** Proposals have timelocks; the upgrade
  gate stops a node that was not voted in for a height; the parameter admin
  can be revoked.

What this does **not** cover, and why the rest of this plan exists:

- the box itself (kernel, SSH, nginx, Postgres, the other site on it);
- the serving layer (explorer, read API, faucet), which is derived data and
  is only as available as the machines serving it;
- liveness with fewer than four validators;
- the accounts that control everything (Cloudflare, GitHub, the VPS
  provider) and the person holding them.

## 3. Replicas: why not Patroni and etcd

Patroni + etcd is the right answer when Postgres is the system of record
and losing the primary loses data. Neither holds here:

- **Postgres is not the system of record.** It holds the indexer's read
  model, rebuilt from any node by replaying the chain (`--backfill-from 0`),
  plus the faucet's claim table, whose loss means a few extra claims. The
  chain's own replication (section 2) already keeps N copies of the truth.
- **Two nodes cannot elect safely.** etcd needs a majority; with two
  members a partition has no majority and Patroni correctly refuses to
  promote. Automatic failover needs at least three DCS members, which is a
  third box doing nothing else.
- **It does not fit the budget.** The indexer database is a 256 MB
  container on a 4 GB box. Patroni, etcd and the replication stream would
  be the largest thing on the host, protecting the least important data.

What I do instead: **each box runs its own node, its own indexer and its
own Postgres, and the indexer follows the local node.** Both databases are
independently derived from the chain, so they agree without ever talking
to each other. There is no replication link to secure, nothing to promote,
and a new box is seeded by a deploy plus a backfill (or by restoring the
daily `pg_dump` from the backup, which is already encrypted and shipped
off-box). The faucet's claim table lives on one box; the other box proxies
faucet claims to it over the mesh and only takes them locally when the
owner is unreachable (section 4).

If a client ever needs Postgres as a system of record for its own data,
that is the client's database and the client's problem; it is not the
chain's.

## 4. Topology: two boxes now, four later

The honest sizing table, from `n = 3f + 1`:

| Boxes | Validators | Survives | What the extra box buys |
|---|---|---|---|
| 1 | 1 | nothing | the current testnet |
| 2 | 1 + follower | box B lost: nothing; box A lost: chain pauses, reads and the sites stay up from B | serving continuity, a live state replica, a second origin |
| 2 | 2 | nothing, and now *either* box pausing the chain | strictly worse liveness than one validator; do not do this |
| 4 | 4 | one box lost, chain keeps producing | real fault tolerance |

So the second box is **a follower, not a validator**, until there are four.
Two validators would make the chain depend on both boxes being up. Four
validator processes split two per box would survive a process crash but not
a box loss, which the restart policy already covers; skip it.

**If I run a validator on box B anyway.** It is a defensible choice for a
testnet, and it is the choice if the second box is mainly there to spread
key material: a second observer, a second signer host for a real 2-of-3
with the third share off the boxes (section 5), and a second vote on
checkpoints. The price is liveness: with `n = 2` the chain pauses whenever
*either* box is down or rebooting for updates, where with a follower it
pauses only when box A is. Nothing else changes: box B serves reads and
writes the same way, the tunnel fails over the same way, and the chain is
equally safe (it never forks either way). The deploy's host role decides
which it is, so the choice can be made per reset and reversed. My
recommendation stays follower for liveness, validator if I would rather
rehearse the multi-host key split before there are four boxes.

**Box A (host 0)** keeps the validator, the observer, the signer, the
shared signet `bitcoind`, the checkpoint proposer, an indexer with its own
Postgres, nginx and a tunnel connector.

**Box B** runs a follower node (`--role follower`, listed in the
validators' `--extra-peers` so it is always connected), an indexer with its
own Postgres, nginx with the same web roots, and a second tunnel connector.
No observer, no signer, no vault material, no Bitcoin node (the follower
does not need one). It serves every read and accepts action submissions:
the node admits them to its mempool and gossips them to the validator, so
`POST /rpc/v1/actions` works on either box.

**Four boxes** later: validators 0 to 3 with observers and signers (2-of-3
or 3-of-4), every box an origin, and the follower role kept for public
"sentry" nodes that third parties peer with so the validators never need a
public port (section 6).

**The deploy learns a role.** `DEPLOY_HOSTS` entries get
`"role": "validator" | "follower"`. The genesis job lists only validators;
the deploy job skips the observer, signer and checkpoint units on a
follower, passes `--role follower`, and renders the followers' keys into
the validators' `--extra-peers`. The web roots and nginx go to every host
with `"edge": true` instead of only host 0. The faucet secret goes only to
the host marked `"faucet": true`.

**Serving on both boxes without a leader.** Both connectors register with
the same Cloudflare tunnel; Cloudflare spreads new connections over the
healthy connectors and stops using one that disconnects. That is the whole
failover mechanism for the serving layer, and it needs no election because
nothing on the serving path is a singleton: the read API is derived data,
action submission gossips from anywhere, WebSocket clients already resume
by sequence number after a reconnect (`API-WS.md`). A lagging follower is
the one case Cloudflare cannot see, so each box gates its own connector: a
`keel-edge-gate.timer` compares the local node's height with the peer's
over the mesh and with `/v1/ready`, and stops `cloudflared` when the local
node is more than a few blocks behind for more than a minute, restarting
it when it catches up. The gate never stops the last healthy connector;
if both are unhealthy, stale reads beat no reads. The optional upgrade is
Cloudflare Load Balancing with health monitors per origin, which does the
same from the edge for a small monthly fee; the gate is enough to start.

**Singletons use the chain as their lock.** The jobs that must not run
twice are the checkpoint proposer, client re-onboarding after a reset and
the faucet. The proposer already goes through governance: a proposal for a
height either exists or it does not, so `btc-checkpoint.sh propose` runs on
every validator host with a per-host offset and skips when an open proposal
for that height is already on chain. No etcd; the ledger is the lock.
Re-onboarding is a reset-time job and stays on host 0 of that deploy. The
faucet stays a singleton by configuration (`"faucet": true`), with the
other edge proxying to it; the claim history is not worth more.

## 5. Keys on the boxes: what is threshold and what is not

### 5.1 Who owns what

The parts, by what each one holds and who is meant to own it:

| Part | What it is | What it holds | Owner by design | Owner today |
|---|---|---|---|---|
| Node (`keel-node`) | the chain software; every node holds the full state and serves RPC | a p2p identity; as a follower, nothing of value | anyone who wants a copy: operators, clients, sentries | me, one |
| Validator | a node whose consensus key is in the validator set, with a bond | the consensus key (hot, unavoidably), the locked bond, the rewards | each validator operator owns its own key and bond and is slashed for its own faults; the chain belongs to no one of them | me, one, and the same key holds the float and the admin roles |
| Observer (`keel-observer`) | watches Bitcoin and Tron and the chain; attests deposits; builds payouts and asks the signer to sign them | the observer key, with a bond at stake; no funds | each observer operator; the quorum, not any one of them, credits a deposit | me, same key as the validator |
| Signer (`keel-tss`) | holds one share of the vault key; signs only payouts the chain authorised | one share | nobody owns the vault key: it exists only as the set of shares, held by separate operators; `t` of them act together | me, and in `local` mode the whole key, not a share |
| Vault | an address on Bitcoin, Ethereum or Tron | the backing for users' balances, 1:1 by the reserve invariant | the users, as balance claims; the signer set controls movement; a model-A client owns its own vault and signer outright | me |
| Indexer and its Postgres | a read model derived from a node; the explorer API and the faucet | nothing of value except the faucet key | anyone; clients run their own | me |
| The serving layer | nginx, the explorer, the site, the tunnel: "the app" from outside | no keys | the operator; it can vanish and the chain is unaffected (section 14.1 of `how-it-works.md`) | me |
| Keel Wallet | the browser extension and `window.keel` | the user's seed, never leaving the browser | the user; the chain never custodies it | the user |
| Clients | front ends and services built on the chain | their own keys: attester, parameter admin if granted, their own vault in model A, their fee account | each client; Keel owns the rails, the ledger and the explorer, never a client's margin | one sandbox client, keys on its own side |
| Governance | proposals, timelocks, the treasury and DAO buckets, the roles | the treasury and buckets are keyless system accounts | KEEL holders through proposals; the parameter admin, attester and arbitrator are delegated and revocable roles | the three roles default to host 0's key |
| Infrastructure | the boxes, Cloudflare, GitHub, the provider | every key above, as files | the operator, and never the chain's data: any node rebuilds it | me |

Read down the "owner today" column and the plan writes itself: the design
puts the validator, observer, signer, vault, governance roles and serving
layer in different hands, and today they are one key on one box. The rest
of this section and section 4 are how they come apart, in the order that
matters: the vault key into shares, the admin roles off the box, the
money off the consensus key, then the boxes themselves to other operators
when there are validators who are not me.

### 5.2 What a box holds today

The intent is that no box can move funds on its own: external payouts go
through threshold signing, and nothing else of value sits behind a key a
box holds. That is the design. Here is what a box actually holds today,
read from the deploy script rather than from the design:

| Key on the box | Put there by | Moves | Threshold? |
|---|---|---|---|
| Vault signer seed (`KEEL_SIGNER_SEED`, rendered as `tss_url = "local:…"` in the observer config) | the deploy, `local` mode | every Bitcoin and Tron vault payout: the whole vault | **No.** The live testnet runs the development signer because there is one box. The threshold signer (`keel-tss`, CGGMP21, 2-of-3 exercised nightly) exists and is the deploy's default mode; it is not what is running. |
| Validator seed | the deploy | signs blocks; is also the validator's account: the bond, a 1,000,000 KEEL float, on host 0 a 1,000,000 KUSD float, and the rewards; signs observer attestations with the same key; and, because the three role variables are unset, it is the parameter admin, the attester and the arbitrator | **No, and block signing never can be.** Everything else attached to it can come off. |
| Faucet secret | the deploy, or generated on host 0 | the faucet balance (50,000 KEEL and 50,000 KUSD) | No; hot by design |
| TSS share and `KEEL_TSS_PASSPHRASE` (`tss` mode) | the ceremony and the deploy | one share of the vault key; the passphrase that decrypts it sits in the same box's env file | Yes: one share moves nothing. The passphrase protects backups and a stolen disk, not a running box. |
| Bitcoin Core wallet | the observer | nothing: created with `disable_private_keys` | watch-only, as it should be |
| Treasury, community and team buckets | genesis | only by `TreasurySpend` proposals; system accounts, no key anywhere | keyless, as it should be |
| Lightning (LND) | not deployed on the testnet | the pool balance when it is | hot by nature, bounded by the pool cap |

So: the vault is threshold in design and single-key in fact; the chain-side
funds (float, rewards, faucet, the KUSD onboarding float) sit behind plain
hot keys; and the on-box key wears every admin hat. One root shell on
host 0 today is the vault, a million KEEL, a million KUSD, the faucet and
the parameter admin. That is the testnet, where none of it has value, but
it is not the shape mainnet can inherit.

What changes:

1. **Run the threshold signer, with one share off the boxes.** `tss` mode
   needs `t` signers online per payout, not `n`. With two boxes: `n = 3`,
   `t = 2`, the third share held by a signer I run elsewhere (a small box
   that does nothing else, or my own machine, online only for a ceremony
   or a recovery). Box A and box B sign routine payouts; one compromised
   box holds one share and moves nothing. With four boxes: 3-of-4. The
   policy check stays on (`keel-tss serve --policy-rpc`): every signer
   verifies against the chain that the payout it is asked to sign is one
   the chain authorised, so an attacker needs `t` compromised *signers*,
   not `t` copied *shares*.
2. **Separate the consensus key from the money.** The validator record
   gets a `rewards_to` address, set at registration and changeable by the
   validator's key after a delay; rewards and the bond on exit go there,
   never to the consensus account. The genesis float shrinks to what a
   validator needs to submit its own actions (votes and attestations are
   system actions already), not a million KEEL, and the KUSD onboarding
   float moves to an off-box onboarding key. Target: the account behind a
   consensus seed holds budget capacity and nothing worth stealing.
3. **Separate the observer key from the consensus key.** Today they are
   one seed. Attestation has its own blast radius (false Tron deposits up
   to the quorum; Bitcoin and Ethereum deposits are proof-checked and
   cannot be invented by a key), so it gets its own seed per host
   (`KEEL_OBSERVER_SEED_<i>`) and the quorum becomes 2-of-3 as soon as
   there are three observers.
4. **The admin hats come off the box now.** `KEEL_PARAM_ADMIN`,
   `KEEL_ATTESTER` and `KEEL_ARBITRATOR` get set to keys I hold off the
   boxes before the next reset; today they default to host 0's account.
   A variable change and a reset, nothing to build.
5. **Cap the faucet.** It stays hot; its balance is the blast radius. It
   holds a week of claims and a scheduled transfer from an off-box key
   tops it up. Today's balance is far more than a week.
6. **The passphrase stays, for what it is.** A running signer must read
   its share, so the passphrase cannot leave the box that signs. Sealing
   it as a systemd credential (section 8) keeps it out of the process
   environment and away from every other process, which is the realistic
   gain.

What threshold signing does not solve, and I should not pretend it does:
the consensus key has to be hot, so a compromised validator can
double-sign (slashing is on the mainnet list and the damage is bounded by
the bond); the faucet and Lightning are hot by nature and are bounded by
their balances; and `t` compromised boxes *are* the vault, which is why
the boxes get the hardening in section 8 and why the fourth box should be
on a different provider, or at least a different network, from the
others.

## 6. Zero-trust edge: Cloudflare Tunnel and Access

The principle: **no box listens on the public internet.** Everything that
must be reachable goes out through an authenticated tunnel to Cloudflare,
and everything administrative goes through Cloudflare Access with an
identity check. After this section the only public port on any box is
WireGuard's UDP port, which does not answer unauthenticated packets at all.

**Web.** `cloudflared` runs as a systemd unit on every edge box, connected
to one named tunnel with ingress rules for `keelchain.com`, `www`,
`testnet` and `faucet`, each to nginx on loopback. nginx stops listening on
public interfaces; the firewall drops 80 and 443. The origin certificate
becomes unnecessary (the tunnel is TLS to Cloudflare; loopback is
loopback), which also removes the one manual step a new edge box needed.
The deploy installs `cloudflared` and its token from a new secret
(`CLOUDFLARE_TUNNEL_TOKEN`) the way it installs everything else. nginx
trusts only loopback as a proxy and takes the client address from
`CF-Connecting-IP`, which fixes the rate-limit keying in section 1.

**SSH.** Port 22 closes. The tunnel gets an `ssh.<domain>` route to
`localhost:22` on each box, protected by an Access application with two
policies: my identity (one-time PIN to my mailbox, or GitHub SSO) for me,
and a **service token** for the deploy workflow. My `~/.ssh/config` uses
`ProxyCommand cloudflared access ssh --hostname %h`; the workflow installs
`cloudflared` on the runner and sets `TUNNEL_SERVICE_TOKEN_ID` and
`TUNNEL_SERVICE_TOKEN_SECRET` from two new environment secrets. Every
session is logged in the Access log with who and when. The break-glass
path if Cloudflare itself is down is the provider's console, with the
password kept off any machine.

**Admin interfaces.** Grafana, Prometheus and Alertmanager get Access
routes too (`grafana.<domain>`, identity policy, my email only). Nothing
administrative is ever a public hostname without Access in front.

**Consensus traffic.** Today the consensus port is public because
validators on different boxes need to reach each other. They all share the
WireGuard mesh, so the node advertises its mesh address instead, the
bootstrappers are rendered with mesh addresses, and the public consensus
port closes. When third parties want to run followers, they peer with a
sentry (a follower box with the consensus port open and nothing else on
it), never with a validator. This is the standard sentry layout; it means a
flood aimed at the chain's p2p layer hits a disposable box.

**Origin hygiene.** Every DNS name on a box is proxied; the other site on
host 0 is proxied (or moved to its own box, which is where it belongs). I
assume the current address is already known, so the firewall has to hold on
its own, and it does once nothing listens. Outbound connections (Bitcoin
peers, the Tron API, SMTP for alerts) reveal the address to those peers
only; acceptable.

**What Cloudflare now absorbs.** Volumetric floods (L3/4) never reach the
box: nothing on it accepts a packet from the internet except WireGuard.
HTTP floods (L7) are shaped at the edge by section 7 before any connection
reaches the tunnel. The provider's own network is the remaining exposure,
and that is the provider's job.

## 7. WAF, rate limits and flood control at three layers

Each layer catches what the one above cannot see.

**Edge (Cloudflare).**

| Rule | Where | Effect |
|---|---|---|
| Managed ruleset on | WAF | common exploit signatures dropped before the origin |
| Bot Fight Mode on for `keelchain.com` and `faucet` | Bots | scripted faucet draining gets a challenge |
| No challenges on `/api/` and `/rpc/` of `testnet` | custom rule, skip | API clients and the wallet extension cannot solve a challenge; these paths rely on rate limits instead |
| Block methods other than `GET`, `POST`, `OPTIONS` | custom rule | nothing else is used |
| Block bodies over 64 KB on `/rpc/` and `/api/` | custom rule | an action is a few hundred bytes |
| `POST /rpc/v1/actions`: 60 per minute per IP | rate limiting | burst above the budget system's own pacing |
| `POST /api/v1/faucet`: 10 per 10 minutes per IP | rate limiting | the faucet's own cooldown is per address; this is per address pair |
| `GET /api/*`, `/rpc/*`: 600 per minute per IP | rate limiting | explorer polling is a few per second at most |
| Cache `GET /api/v1/blocks/<n>`, `/api/v1/tx/<hash>`, `/api/v1/receipts/<hash>` for an hour | cache rule | immutable once final; read floods hit the cache, not the indexer |
| Never cache `/v1/status`, `/v1/ready`, accounts, books | cache rule | live data |

The plan's rate-limiting rule quota decides how many of these are edge
rules; whatever does not fit stays in nginx, which has the same rules
already. The point of duplicating them at the edge is that a flood then
costs Cloudflare's capacity, not the tunnel's.

**Origin (nginx).** Real client IP from the tunnel; `client_max_body_size
64k` on the API locations; `limit_conn` per address (20 connections, which
covers an explorer tab and a wallet); the WebSocket locations split out
with their own connection zone and the long read timeout, everything else
on a 30 s timeout; `server_tokens off`; a default server that returns 444
to any unknown `Host`; the API-key gate kept for action submission; a
`maintenance` flag file that turns writes into 503 while reads continue
(section 9).

**Application.** The node RPC gets a request body limit (64 KB) and a
WebSocket subscription cap per connection; the mempool gets a bound per
address and a total bound so an authenticated peer or a local flood cannot
grow it past memory (the per-address budget already bounds what can be
*included*, this bounds what can be *queued*). The indexer's Postgres gets
`statement_timeout = 5s` and a small `max_connections`; the indexer's page
sizes are already capped. Units get `CPUQuota` and `TasksMax` next to the
`MemoryMax` they already have, so one flooded service cannot starve
consensus.

## 8. Host hardening

- **Services drop root.** A `keel` system user; `User=keel`,
  `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ProtectHome=yes`,
  `PrivateTmp=yes`, `ReadWritePaths=/var/lib/keelchain`,
  `CapabilityBoundingSet=` on every unit. Config files move from root-only
  to `keel`-readable, mode 0400. Secrets move from `EnvironmentFile=` to
  systemd credentials (`LoadCredentialEncrypted=`, sealed to the host), so
  they are not visible in `/proc` or to a process that is not the unit.
- **The validator box runs the validator.** The other site on host 0 is a
  separate trust domain; a bug in it is a path to the signer. It moves to
  its own box as part of this plan; until then it runs as its own user with
  the same sandboxing.
- **SSH.** Key-only, `PermitRootLogin no`, `AllowUsers` the deploy user,
  reachable only through Access (section 6). The deploy key stays
  single-purpose and tagged; it rotates by replacing the secret and the
  `authorized_keys` line in one deploy.
- **Kernel.** `net.ipv4.tcp_syncookies=1`, `rp_filter=1`, ICMP redirects
  off, conntrack sized for the mesh plus the tunnel's connections; the
  existing buffer sizes stay. `unattended-upgrades` for security updates
  with automatic reboots in a window, because a validator that reboots
  pauses the chain for a minute and an unpatched one is worse.
- **Time.** `chrony` on every box; consensus timeouts assume clocks within
  a second.
- **Postgres** stays bound to loopback inside its memory-limited container;
  the password stays in the container's environment and the indexer's
  credential, nowhere else.
- **Logs.** journald is already capped; Access, Cloudflare audit and GitHub
  audit logs are the off-box record of who did what.

## 9. Switches: what I can flip, and who can flip it

An incident switch is useful only if it is written down with its blast
radius and its reversal before it is needed, and only if nobody else can
flip it. Both halves below.

### 9.1 The switches

| Switch | Where | Stops | Keeps | Reverts by | Takes effect |
|---|---|---|---|---|---|
| Block writes at the edge | Cloudflare WAF rule "block `POST /rpc/v1/actions`", toggled on | new actions from the internet | reads, explorer, sites, consensus | toggling off | seconds |
| Under Attack mode | Cloudflare, scoped to `keelchain.com` and `faucet` by a configuration rule | scripted traffic to the sites | the API hosts, which must never get a challenge | turning off | seconds |
| Detach an origin | `systemctl stop cloudflared` on the box, or the edge gate | that box serving anything | the other box serving everything | `systemctl start cloudflared` | seconds |
| Maintenance (writes) | `touch /run/keelchain/maintenance` on an edge box | action and faucet submission on that box (503) | reads | removing the file | immediate |
| **Payout freeze** | `systemctl stop keel-tss@<i>` on `n - t + 1` hosts (on today's box, the one signer) | every external payout: no threshold can be reached, so nothing leaves a vault on any network | the chain, deposits, trading, withdrawal *requests* queue on chain | starting the signers | immediate |
| Deposit freeze | `systemctl stop keel-observer@<i>` on enough hosts to break the attestation quorum | crediting of new deposits | everything else | starting the observers | immediate |
| Budget to minimum | `SetParam` from the parameter admin: `per_locked_keel_per_day` to its floor | every non-free action economically, chain-wide, including from a validator's own RPC | consensus, reads, governance | `SetParam` back | next block |
| Chain pause | stop the validator units | everything (safe: no block is lost, no fork) | nothing | start them | immediate |
| Revoke a client | governance proposal replacing the attester or parameter admin | that client's privileged role | its users' funds | a new proposal | timelock |
| Revoke a deploy | GitHub environment: rotate `SSH_PRIVATE_KEY`, remove the key line on the boxes | the compromised key | every other path | nothing to revert | minutes |
| Freeze deploys | GitHub environment: required reviewer, or disable the workflow | any push reaching the boxes | the running chain | re-enabling | immediate |

The **payout freeze** is the one that matters most and the only one that
is immediate, off-chain and does not need a key: stopping the signer
daemons makes the vaults inert while the ledger keeps running. It is the
first thing to flip when anything about custody looks wrong, and the drill
in section 11 proves it works before it is needed. Everything else on the
chain is a ledger entry that governance can reason about later; an
external payout is the only thing that cannot be taken back.

Two chain-side switches do not exist yet and are added by this plan: a
per-network **withdrawal pause parameter** (`vaults.<chain>.paused`, set by
the parameter admin, revocable by governance) so a freeze on one network
does not require stopping every signer, and an alert that fires when any
switch above is in its "stopped" state for more than ten minutes, so a
flipped switch is never forgotten.

### 9.2 Protecting the switches

Everything above is controlled from three accounts and one key. Each gets
the same treatment:

- **Cloudflare.** Hardware-key two-factor; no global API key in use; one
  scoped API token per purpose (today only the tunnel), stored only in
  the GitHub environment; audit log
  notifications to my mailbox; no other members until there is a reason,
  and then with roles, not Super Administrator.
- **GitHub.** Hardware-key two-factor on the account and required on the
  organisation; the `testnet` environment restricted to `main` with me as a
  required reviewer, so a merged commit does not reach the boxes until I
  click; branch protection on `main` (no force push, signed commits); the
  deploy scripts never print a secret and CI fails on the names that must
  stay out of the repo; Dependabot alerts on.
  `KEEL_RESET_CHAIN` is the one variable that destroys data: it is set
  only in the same sitting as the run and deleted after, and the deploy
  will log it loudly so a stale value cannot wipe a chain by surprise.
- **The VPS provider.** Hardware-key two-factor; the console password off
  every machine; this is the break-glass path and gets the same care as
  the keys.
- **The parameter-admin key.** Held off the boxes, used from my laptop
  through the CLI, never in the environment. Its power is bounded by the
  parameter validation ranges and revocable by governance; still, it is
  the only key that can change the chain's economics in one block, so it
  lives in a hardware-backed store once the CLI supports one.
- **Alerting on the switches themselves.** Every state in 9.1 that is not
  the normal state raises an alert while it holds (unit stopped, maintenance
  file present, connector detached, WAF block rule on, deploys frozen).
  Nobody can flip a switch quietly, including me by accident.
- **No two-person rule yet.** I am one operator; the compensating controls
  are the timelocks on everything on chain, the alert on every switch, and
  the audit logs off the box. The mainnet plan adds a second key holder.

### 9.3 Automatic switches

Some switches should not wait for me. The rule: a switch flips itself only
when the safe direction is "closed", the trigger is a signal I already
alert on, and the flip itself raises the alert so I know. Each automatic
flip has a cooldown and a manual override file that stops it from
flapping or from re-flipping while I am investigating.

| Trigger (existing alert) | Automatic action | Reverts |
|---|---|---|
| Edge-side DDoS detection | nothing to do: Cloudflare's volumetric and HTTP mitigation is always on and needs no switch | itself |
| Origin saturated: p95 latency or 5xx rate from the edge over a threshold for two minutes (Cloudflare notification, or the node's own `/v1/status` latency from the probe) | the write-block WAF rule on, via a scoped Cloudflare API token from an Alertmanager webhook on host 0; sites and reads keep serving from the cache | off automatically after fifteen quiet minutes, or by hand |
| Custody disagreement: a reserve-invariant failure, an outbound the observers disagree on, or `KeelHostsDisagree` | **payout freeze**: the signer unit stops on this host (each host does this for itself, so `n - t + 1` stopping is reached without coordination); the chain-level per-network pause parameter when it exists | never automatically; a payout freeze lifts by hand after the cause is known |
| Follower behind the validator by more than a few blocks for a minute | the edge gate detaches this box's connector (section 4) | automatically when it catches up |
| Validator unit restarting in a loop | stop the connector on that box so clients get the healthy box; the chain is already paused if it was the only validator | automatically when the unit is stable for five minutes |
| A switch left in its non-normal state for more than ten minutes | no action: a reminder alert every hour until it is reverted | by hand |

The webhook receiver is a small script behind Alertmanager on host 0 with
one job per alert name; the Cloudflare token it holds can toggle exactly
one WAF rule and nothing else. Automatic opening never happens for custody.
The drills in section 11 include firing each trigger on the staging box
and watching the flip and the revert.

## 10. Incident playbooks

Evidence first, then the switch. Every alert names a probe to run before
anything is flipped.

**HTTP flood or scraping.** Confirm in Cloudflare analytics (requests per
path, top sources). Edge rate rules should already be holding; if the
origin is still saturating, turn on the write-block rule, then Under Attack
mode for the sites. Reads keep working from the cache. Nothing on the chain
changes. Revert when the analytics are quiet.

**Flood of valid actions.** Budgets drain the sender; if the mempool still
grows, lower the per-key rate in the API-key map and, as a last resort,
`SetParam` the capacity down to its floor. Check the sender's address on the explorer
first; a legitimate client doing a backfill looks the same.

**A box is unreachable.** Confirm from the other box over the mesh and from
the provider's status page. If box A is gone, the chain is paused and the
sites are up from box B; nothing to flip, wait or rebuild. If box B is
gone, nothing is affected. Rebuild a box with `prepare-host.sh` and a
deploy; a follower backfills from the validator, an indexer from its node
or from the last `pg_dump`.

**Something about custody looks wrong** (an outbound I do not recognise, a
reserve alert, an observer disagreeing). The payout freeze has already
fired on its own (9.3); confirm it has, then look. Deposits can keep flowing. Investigate against the
explorer and the Bitcoin node; unfreeze only when the cause is understood.

**A key leaked.** What rotates, by key: the deploy key (new secret, one
deploy, the old `authorized_keys` line gone); an API key (new map, one
deploy); the tunnel token (new tunnel, old deleted); the signer passphrase
or a share (new ceremony, funds swept to the new vault key, old vault
retired); a validator seed on the testnet (a new chain; on mainnet, a key
rotation action, which is on the mainnet list); the parameter admin
(governance replaces it). The CI guard and this list are why no secret is
ever printed, even partially.

**A box is compromised.** Detach it (stop its connector, revoke its mesh
peer in the next deploy, payout freeze if it held a share), keep the disk
for reading, rebuild from scratch, rotate every secret that was on it, then
restore state from a verified snapshot. A compromised validator cannot
rewrite history (section 2); it can double-sign, which is why slashing is
on the mainnet list.

**Cloudflare is down.** Nothing is reachable and that is the design; there
is deliberately no direct-to-origin fallback because it would be a
permanent hole for a rare outage. The chain keeps running over the mesh.
Wait.

## 11. Order of work

1. **Hygiene, hours.** Proxy the other site's DNS. Real-IP in nginx. Body
   size caps. Grafana off port 3000. Hardware keys on the three accounts.
   Required reviewer on the `testnet` environment.
   Set the three role variables to off-box keys so the next reset takes
   the admin hats off host 0.
2. **Services off root, a day.** The `keel` user, sandboxed units,
   credentials instead of environment files, `chrony`, kernel sysctls,
   unattended upgrades. Drill: payout freeze and unfreeze on the staging
   box.
3. **The tunnel, a day.** `cloudflared` on host 0, nginx on loopback, SSH
   through Access for me and for the deploy workflow, admin routes behind
   Access, consensus on the mesh, then close 22, 80, 443 and the consensus
   port. Verify from outside that nothing but WireGuard answers.
4. **Box B, a day.** Host roles in the deploy, the follower, its indexer and
   Postgres, the second connector, the edge gate, faucet proxying, the
   checkpoint proposer made idempotent. Drill: stop box A's connector and
   watch the sites stay up; stop box A and watch the chain pause and
   resume. Then `tss` mode: a 2-of-3 ceremony with the third share
   off the boxes, the observer seed split from the validator seed, the
   faucet cut down to a week of claims.
5. **Chain changes for the keys, a few days.** `rewards_to` on the
   validator record, a smaller genesis float, the off-box onboarding key,
   the withdrawal pause parameter and the switch alerts.
6. **WAF and rate rules, hours.** The table in section 7 at the edge, the
   cache rules, the write-block rule left off and ready.
7. **Four boxes, when the budget allows.** Validators 1 to 3 with observers
   and signers, a real `t`-of-`n` ceremony, sentries for third parties.
   This is the step that turns "safe" into "safe and always on".

Cost: the tunnel, Access for one user, the managed WAF ruleset, basic rate
limiting and DDoS protection are on Cloudflare's free plan; Load Balancing
with health monitors is the one optional paid piece. Box B is the only new
infrastructure until step 6.
