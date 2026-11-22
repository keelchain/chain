# Install the command-line tools

Keel ships five programs. Most people need only the first.

| Program | What it is for |
|---|---|
| `keel` | The CLI: read accounts and the chain, sign and send any action (transfers, orders, locks, plans, custody, governance) |
| `keel-node` | A node: a follower that verifies the chain and serves the RPC, or a validator |
| `keel-indexer` | The read API behind the explorer; needs PostgreSQL |
| `keel-observer` | The observer and signer daemon that watches the external chains |
| `keel-tss` | The threshold signer a custodial client runs for its own vault (Linux and macOS) |

They are built for Linux (x86_64, arm64), macOS (Apple silicon, Intel) and
Windows (x86_64) by the `Release command-line tools` workflow from the
public source, and published with their SHA-256 checksums.

## Install

<div class="tabs" data-group="os">
<div class="tab" data-mode="linux">

One command. It detects your processor, checks the download against its
checksum and puts the programs in `~/.local/bin`:

```
curl -fsSL https://keelchain.com/install.sh | sh
```

Or by hand:

| Processor | Download |
|---|---|
| x86_64 (Intel, AMD) | [keel-linux-x86_64.tar.gz](https://keelchain.com/downloads/keel-linux-x86_64.tar.gz) |
| arm64 (Graviton, Ampere, Raspberry Pi 4 and later, 64-bit) | [keel-linux-arm64.tar.gz](https://keelchain.com/downloads/keel-linux-arm64.tar.gz) |

```
curl -fLO https://keelchain.com/downloads/keel-linux-x86_64.tar.gz
curl -fLO https://keelchain.com/downloads/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf keel-linux-x86_64.tar.gz
install -m 755 keel-linux-x86_64/keel* ~/.local/bin/
```

Needs glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 and later).
If `keel` is not found afterwards, add the directory to your `PATH`:
`export PATH="$HOME/.local/bin:$PATH"` in `~/.bashrc` or `~/.zshrc`.

</div>
<div class="tab" data-mode="macos">

One command, in Terminal. It detects Apple silicon or Intel, checks the
download against its checksum and puts the programs in `~/.local/bin`:

```
curl -fsSL https://keelchain.com/install.sh | sh
```

Or by hand:

| Mac | Download |
|---|---|
| Apple silicon (M1 and later) | [keel-macos-arm64.tar.gz](https://keelchain.com/downloads/keel-macos-arm64.tar.gz) |
| Intel | [keel-macos-x86_64.tar.gz](https://keelchain.com/downloads/keel-macos-x86_64.tar.gz) |

```
curl -fLO https://keelchain.com/downloads/keel-macos-arm64.tar.gz
curl -fLO https://keelchain.com/downloads/SHA256SUMS
shasum -a 256 --check --ignore-missing SHA256SUMS
tar -xzf keel-macos-arm64.tar.gz
mkdir -p ~/.local/bin && install -m 755 keel-macos-arm64/keel* ~/.local/bin/
```

The programs are not signed with an Apple developer certificate. Downloaded
with `curl` as above they run as they are. If you downloaded the archive
with a browser, macOS quarantines it and refuses to open the programs;
clear that once with `xattr -dr com.apple.quarantine ~/.local/bin/keel*`.
If `keel` is not found, add `export PATH="$HOME/.local/bin:$PATH"` to
`~/.zshrc`.

</div>
<div class="tab" data-mode="windows">

One command, in PowerShell. It checks the download against its checksum,
puts the programs in `%LOCALAPPDATA%\Keel\bin` and adds that folder to
your `PATH`:

```
irm https://keelchain.com/install.ps1 | iex
```

Or by hand: download
[keel-windows-x86_64.zip](https://keelchain.com/downloads/keel-windows-x86_64.zip),
check it, and extract it wherever you keep programs:

```
Invoke-WebRequest https://keelchain.com/downloads/keel-windows-x86_64.zip -OutFile keel.zip
Invoke-WebRequest https://keelchain.com/downloads/SHA256SUMS -OutFile SHA256SUMS
(Get-FileHash keel.zip -Algorithm SHA256).Hash.ToLower()   # compare with the line in SHA256SUMS
Expand-Archive keel.zip -DestinationPath .
```

The programs are not code-signed, so Windows SmartScreen may ask before
the first run: choose *More info*, then *Run anyway*. `keel-tss` is not
built for Windows (it needs GMP): run it under WSL with the Linux
install. Open a new terminal after installing so the `PATH` change takes
effect.

</div>
</div>

## Check it works

```
keel --version
keel --rpc https://testnet.keelchain.com/rpc status
```

The second line asks the public testnet for its height and validators, and
needs no key. Every command takes `--help`.

## First steps with `keel`

```
keel keygen                                   # a new key: prints the address and the secret
keel --rpc https://testnet.keelchain.com/rpc --chain-id 3 account <address>
keel --rpc https://testnet.keelchain.com/rpc --chain-id 3 send --secret <secret> --wait 30 lock-budget 1000000
keel --rpc https://testnet.keelchain.com/rpc --chain-id 3 send --secret <secret> --wait 30 transfer <to> KEEL 5000000
```

Amounts are in the smallest unit (1 KEEL = 1,000,000 orcas). A new address
needs KEEL before it can act: claim some at
[faucet.keelchain.com](https://faucet.keelchain.com/), then lock 1 KEEL as
above (`how-it-works.md`, section 7). The secret is a private key: keep it
out of shell history and scripts you share. The testnet's chain id is 3; a
local devnet's is 1.

## Updating and removing

Run the install command again to update; it replaces the programs in
place. [keelchain.com/downloads/VERSION](https://keelchain.com/downloads/VERSION)
is the version being served, and every past release is on
[GitHub](https://github.com/keelchain/chain/releases). To remove, delete
the `keel*` files from the install folder.

## Build from source

On any system with Rust 1.98 or newer and `m4` (for GMP, which only
`keel-tss` needs):

```
git clone https://github.com/keelchain/chain
cd chain/chain
cargo build --release -p keel-cli -p keel-node -p keel-indexer -p keel-observer -p keel-tss
```

The programs are in `target/release/`. Leave `-p keel-tss` out on Windows.
A build from the same commit is what the release workflow publishes.
