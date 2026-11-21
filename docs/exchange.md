# Run an exchange on Keel: getting started

This is the short path from nothing to an exchange whose users trade on
Keel. It is the same on the testnet and on mainnet; the links below are the
testnet's. The longer pages behind each step are `models.md` (who holds
the funds), `clients.md` (the integration contract and the pricing) and
`tokenomics.md` (why it is priced this way).

Everything a client manages lives in one place: **Keel Harbor**,
`https://harbor.keelchain.com`. Harbor has no sign-up and no server. You
connect a wallet, it reads your account from the chain, and every change
you make there is one action you sign.

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

You keep your own wallets: a vault key you own, a signer you run, and
users whose balances your vault backs (`models.md`, Model A). The steps
below are the same as for any client; what is yours alone is the vault
registration and the signer, in step 8.

</div>
<div class="tab" data-mode="noncustodial">

Your users hold their own keys in Keel Wallet and their funds in the
network vault (`models.md`, Model B). The steps below are the same as for
any client; what is yours alone is the Connect button and the session keys,
in step 8.

</div>
<div class="tab" data-mode="user">

This page is for exchanges. If you are a person with Keel Wallet who wants
to trade on Keel yourself, there is nothing to register and no plan to buy:
install the wallet, get 1 KEEL (the faucet on the testnet), lock it from the
wallet's *Action capacity* card, and ask for a deposit address. `deposits.md`
is the page for that, and `wallet.md` for the wallet itself.

</div>
</div>

## What you need

- **One Keel account**, in Keel Wallet (`https://keelchain.com/wallet/`).
  This is your operator account. It is an ordinary account: the one you
  already have works, and you do not need a second wallet for Harbor. Use
  a separate account in the same wallet only if you want to keep the
  business apart from your personal balances.
- **KEEL to lock** for the tier you want: 100,000, 500,000 or 2,000,000.
  It stays yours and comes back three days after you unlock it.
- **USDT** on that account for the plan fee ($300, $1,000 or $3,000 per
  30 days) and for usage beyond what the plan includes.

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

And, for your vault: a key you generate and keep, with a signer the
observers can reach over HTTPS (`keel-tss serve`, or a `POST /sign` hook in
front of your KMS). Nothing of it is handed to Keel; you register the public
key, the chain code and the signer URL.

</div>
<div class="tab" data-mode="noncustodial">

And, for your site: a server that keeps the session keys your users
authorize (scope-limited, never able to move funds) and relays signing
requests to the wallet (`connect-keel-wallet.md`). Optionally an address for
your dispute team, to bond as an arbitrator.

</div>
<div class="tab" data-mode="user">

A wallet user needs 1 KEEL to lock, and nothing on this list. The lock is
a hundred actions a day and comes back three days after unlocking.

</div>
</div>

## The steps

1. **Install Keel Wallet and create or import your account.** On the
   testnet, the faucet (`https://faucet.keelchain.com`) gives a new account
   its first KEEL and USDT.
2. **Open Harbor and connect.** You can look around at once: your
   balances, the three plans, and what each would take from you today.
   Until step 3 is done Harbor says the account is not registered yet, and
   everything except buying a plan and the business tools already works.
3. **Get registered as a client.** A client is an account that governance
   has put in the attester set, because a client vouches for the KYC tier
   of its users and other traders rely on that. On the testnet I onboard
   clients with the `Onboard client` workflow (`testnet.md`), which also
   sends starting KEEL and USDT; write to `support@keelchain.com` with your
   account address. On mainnet it is a `SetAttesters` governance proposal.
   Nothing about your wallet changes: the same account becomes the client.
4. **Fund the account.** Deposit a dollar stablecoin to the address the
   wallet's Receive card gives you (it is credited as USDT), or have USDT
   and KEEL transferred to the account on Keel. Harbor's Funds page can
   buy KEEL with USDT on the chain's own order book.
5. **Lock the tier's KEEL** on Harbor's Funds page.
6. **Buy the plan** on Harbor's Plans page. The fee is taken and the
   period starts in that block. It renews by itself every 30 days at the
   tier you chose; change the tier or stop renewal on the same page.

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

7. **Set your fees and onboard users** (Harbor, Business). You set the
   retail fee your users pay you, and attest each user's account
   (`Attest`) from your backend. From then on your users pay no protocol
   fee and need no KEEL: their actions come out of your plan's daily pool.
   An account you hold in custody signs nothing itself; you sign as
   custodian.
8. **Register your vault and integrate.** Start your signer, register the
   vault on each chain with `RegisterCustodyVault` (the vault tools are in
   Harbor, under Business), hand out addresses in it with
   `RequestCustodyAddress` and pay out with `WithdrawCustody`. Read the
   chain through the read API, sign server-side with `@keelchain/sdk`,
   submit to the node RPC. `clients.md` is the contract; `models.md`
   covers the vault, the float swap that lets a custody balance trade on
   the chain's book, and key rotation.

</div>
<div class="tab" data-mode="noncustodial">

7. **Set your fees and onboard users** (Harbor, Business). You set the
   retail fee your users pay you, and attest each user's account
   (`Attest`) from your backend. From then on your users pay no protocol
   fee and need no KEEL: their actions come out of your plan's daily pool.
   The account you attest is the user's own Keel Wallet address, which
   your site learns when the wallet connects.
8. **Integrate.** Add the Connect button (`connect-keel-wallet.md`): the
   wallet signs a login challenge, then every action your site builds for
   the user, and authorizes a session key so trading needs no popup per
   click. Read the chain through the read API, submit what the wallet
   signs to the node RPC. `clients.md` is the contract; `models.md`
   covers what you hold (roles and session keys) and what you never hold
   (a user's key).

</div>
<div class="tab" data-mode="user">

None of this applies to you: there is no fee to set, nobody to attest
and nothing to integrate. Once you hold 1 KEEL locked, request a deposit
address from the wallet's Receive card, send coins to it, and trade on the
book or in escrow (`deposits.md`). If you later join an exchange, it
attests your address and, while its plan runs, you pay no protocol fee and
need no lock of your own.

</div>
</div>

## What it costs, in one table

| | Tier 1 | Tier 2 | Tier 3 |
|---|---|---|---|
| KEEL kept locked (returned on unlock) | 100,000 | 500,000 | 2,000,000 |
| Plan fee per 30 days, in USDT | $300 | $1,000 | $3,000 |
| Usage included | $300 | $1,200 | $4,000 |
| Actions a day for your users | 10,000 | 50,000 | 250,000 |

Usage is priced per flow: $0.02 per deposit address, $0.10 per withdrawal,
$0.002 per filled order, $0.05 per escrow release, $1 per dispute ruling.
It draws on the included usage first. All of these numbers are governance
parameters and Harbor always shows the live ones.

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

The usage lines a custodial client sees are the deposit addresses it
issues in its vault, its custody withdrawals and its float swaps, and, for
balances swapped onto the chain's book, the fills and releases of its
users. Everything else about the table is the same for every client.

</div>
<div class="tab" data-mode="noncustodial">

The usage lines a P2P client sees are the deposit addresses the network
vault issues to its users, their withdrawals, and the fills, releases and
rulings of their trades. Everything else about the table is the same for
every client.

</div>
<div class="tab" data-mode="user">

A wallet user pays none of this. There is no plan and no usage price;
the user pays the protocol fees on value moved instead: 10 bps on a fill
taken, 1% on an escrow release and the flat fee on a withdrawal, all
governance parameters (`governance.md`). Actions themselves cost nothing
beyond the 1 KEEL lock.

</div>
</div>

## Keeping it running

Harbor's Overview shows the one next thing to do, if there is one: lock
more, add USDT, renew. Two things suspend the benefits, and both are
visible there:

- **Unlocking below your tier's KEEL** suspends the plan until you lock it
  back.
- **Usage you could not pay** is owed; until it is settled your users pay
  the protocol fees again and no new deposit address or withdrawal is
  issued to them.

Nothing is ever taken that is not on the chain as an event and a ledger
posting, and nobody, Harbor included, can charge you outside those rules.
