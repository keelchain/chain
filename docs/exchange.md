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
7. **Set your fees and onboard users** (Harbor, Business). You set the
   retail fee your users pay you, and attest each user's account
   (`Attest`) from your backend. From then on your users pay no protocol
   fee and need no KEEL: their actions come out of your plan's daily pool.
8. **Integrate.** Read the chain through the read API, sign with
   `@keelchain/sdk`, submit to the node RPC. `clients.md` is the contract;
   `models.md` covers running your own custody vault (the vault tools are
   in Harbor, under Business) or letting users hold their own keys.

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
