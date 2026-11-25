# What is posted on Keel

Keel is a public chain. Everything in this page can be read by anyone,
without an account, from the explorer or the API, and it stays readable:
a block is never edited and nothing in it can be deleted later. This page
lists every piece of data each path puts there, and what never goes there.

Pick your path once; the tabs below follow it.

## The rule behind every row

An action is the only way anything gets onto Keel. When an action is
included in a block, three things become public and stay public:

1. **The action itself, in full**: who signed it, its number in that
   account's sequence (the nonce), and every field it carries. If a field
   is free text (an offer's terms, a transfer's memo), the text is public.
2. **What it did**: the receipt, with its events and whether it succeeded.
3. **The balances it changed**: every balance on Keel is public, by
   account and by kind (spendable, locked, in escrow, in custody, on hold).

An account is an address: 64 hexadecimal characters. Keel never stores a
name, an email, a phone number, a document or an IP address for an
account. Who the person behind an address is, only they and their exchange
know.

So the question "what does my path post" comes down to which actions that
path sends, and for whom.

## What each path posts

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

Your users have no key. Each has an account on Keel that you created and
that you sign for as custodian, and their coins sit in your vault. Your
order book, your offers and your matching run in your own systems and are
not on Keel; what reaches Keel is the money moving.

**Each user's account**

| What | Posted | Details |
|---|---|---|
| The account | yes | An address. No name. |
| That it is your user | yes | `Attest` records the address, the tier you gave it, when the tier lapses, and that you are the one vouching. Anyone can see which accounts you vouch for. |
| Its balances in your custody | yes | Per asset: spendable in custody, on hold, and anything it moved to the market side. |
| Its history | yes | Every row below, in order, with block time. |

**Money coming in**

| What | Posted | Details |
|---|---|---|
| A deposit address | yes | `RequestCustodyAddress`: the network, your vault, the user's account, and the address's index. The external address itself is listed with its owner, so anyone can tell that a Bitcoin or Tron address belongs to that account at your exchange. |
| A deposit | yes | The user's account, the asset, the amount, and the external transaction's hash and output. The hash leads to the sender on the external chain's own explorer; Keel does not record the sender itself. |
| A held deposit | yes | That the deposit is on hold (if you screen deposits), and its release: how much, and to which account. A deposit you release to a different account than the user's is visible as that. |
| USDT from several networks | yes | Which network's USDT came in and how much unified USDT it became. |

**Money moving inside your exchange**

| What | Posted | Details |
|---|---|---|
| A transfer between two of your accounts | yes | `TransferCustody`: each leg's sender, receiver, asset and amount, the account it was done for, and the memo. A memo is public text of up to 256 bytes: never put a name, an order reference that identifies a person, or a payment detail in it. |
| A trade between two of your users | partly | Keel sees the settlement, not the trade. If you settle both sides in one action (BTC one way, USDT the other), the two legs are public and the price can be worked out from them. If you settle one side only, only that leg is public. |
| Your offers and order book | no | They live in your database. Nothing about an offer, its price, its payment method or who posted it reaches Keel. |
| The fiat payment | no | Keel never sees a bank transfer, a card payment or a cash deposit. |
| A swap to Keel's market and back | yes | `SwapCustody`: the account, the asset, the amount and the direction. |
| An order on Keel's own book | yes | If a balance trades on Keel's order book: the account, the pair, the side, the price, the size, and every fill with its price, size and fee. |

**Money going out**

| What | Posted | Details |
|---|---|---|
| A withdrawal | yes | The user's account, the asset, the amount, the network, and the full external destination address. |
| Your withdrawal fee | yes | The amount and the account of yours that received it. |
| The network fee | yes | What the payout cost on the external chain, charged to you. |
| The payout's result | yes | Confirmed, with the external transaction's hash, or failed, with what was refunded. |

</div>
<div class="tab" data-mode="noncustodial">

Your users hold their own key in Keel Wallet and their coins are in
Keel's network vault. They sign their own actions, so their offers and
trades are on Keel in full. You vouch for them and collect your fee.

**Each user's account**

| What | Posted | Details |
|---|---|---|
| The account | yes | The wallet's address. No name. |
| That it is your user | yes | `Attest` records the address, the tier you gave it, when the tier lapses, and that you are the one vouching. |
| Whose capacity it uses | yes | That its actions come out of your plan's daily pool. |
| A session key | yes | If the wallet lets your site sign routine actions: the key's address, what it may sign, and when it expires. |
| Its balances | yes | Per asset: spendable, in trade escrow, in open orders, locked. |

**Money coming in and going out**

| What | Posted | Details |
|---|---|---|
| A deposit address | yes | The network and the address's index; the external address is listed with its owner. |
| A deposit | yes | The external transaction's hash and how many observers confirmed it, then the account, the asset and the amount credited. |
| A transfer to another account | yes | Sender, receiver, asset, amount, and the memo (public text, up to 256 bytes). |
| A withdrawal | yes | The account, the asset, the amount and the full external destination address, then the external transaction's hash once it confirms. |

**An offer**

Posted in full, under your exchange's name.

| Field | Posted | Details |
|---|---|---|
| Owner | yes | The user's address. |
| Exchange | yes | Your client address, written by the chain, with your public name. |
| Side, asset | yes | Buying or selling, and which coin. |
| Fiat currency, payment method | yes | For example EUR and SEPA. The method is a label, not the account to pay. |
| Price | yes | A margin over the oracle price, or a fixed price. |
| Limits | yes | Smallest and largest trade. |
| Payment window | yes | How long the buyer has to pay. |
| Country, minimum tier | yes | If the owner set them. |
| Terms | yes | Free text, up to 2,000 characters, public as written. Bank details, phone numbers and names must not go here. |
| Payment instructions | hash only | Where to send the fiat stays off chain, encrypted between the two traders. Keel holds a 32-byte fingerprint of it, which reveals nothing and lets a dispute prove what was agreed. |
| Arbitrators | yes | The addresses the owner named to rule disputes. |
| State | yes | Created at which block, paused, closed, and the refundable KEEL deposit. |

**A trade**

| What | Posted | Details |
|---|---|---|
| The two parties | yes | Buyer's and seller's addresses. |
| The amounts | yes | The crypto amount, the fiat amount and currency, the price margin, and the fee. |
| The timeline | yes | Started, marked paid, released or cancelled (and by whom), with the payment deadline. |
| The escrow | yes | The seller's coins sitting in escrow for the trade. |
| Your retail fee | yes | The amount, the payer, and that it was paid to you. |
| The fiat payment itself | no | Keel sees "marked paid", never the payment. |
| Chat | no | End-to-end encrypted, off chain. |
| A dispute | partly | Who opened it, the arbitrator bound to it, and the ruling: how much went to the buyer and how much to the seller. Evidence is posted as a fingerprint only; the files stay off chain. |

**An order on Keel's book**

| What | Posted | Details |
|---|---|---|
| The order | yes | The account, the pair, the side, the price and the size. |
| Each fill | yes | Price, size, the fee, and the order it matched. |

</div>
<div class="tab" data-mode="user">

You hold your own key in Keel Wallet and no exchange stands between you
and the chain. Everything you sign is posted as you signed it.

**Your account**

| What | Posted | Details |
|---|---|---|
| The account | yes | Your wallet's address. No name. |
| Your lock | yes | How much KEEL you locked for capacity, and any unlock on its way back. |
| Your balances | yes | Per asset: spendable, in trade escrow, in open orders, locked. |
| An exchange | no | Nothing links you to one. Your offers show no exchange. |

**Money coming in and going out**

| What | Posted | Details |
|---|---|---|
| A deposit address | yes | The network and the address's index; the external address is listed with your account. |
| A deposit | yes | The external transaction's hash, then your account, the asset and the amount credited. |
| A transfer | yes | Sender, receiver, asset, amount, and the memo (public text, up to 256 bytes). |
| A withdrawal | yes | Your account, the asset, the amount and the full external destination address, then the external transaction's hash. |

**An offer**

| Field | Posted | Details |
|---|---|---|
| Owner | yes | Your address. |
| Exchange | none | Shown as none. |
| Side, asset, fiat currency, payment method | yes | The method is a label, not the account to pay. |
| Price, limits, payment window | yes | |
| Country, minimum tier | yes | If you set them. |
| Terms | yes | Free text, up to 2,000 characters, public as written. Do not put bank details, a phone number or your name here. |
| Payment instructions | hash only | They stay off chain, encrypted between you and the other trader. Keel holds a fingerprint only. |
| State | yes | Created at which block, paused, closed, and your refundable KEEL deposit. |

**A trade**

| What | Posted | Details |
|---|---|---|
| The two parties | yes | Both addresses. |
| The amounts | yes | Crypto amount, fiat amount and currency, price margin, and the protocol fee you paid as seller. |
| The timeline | yes | Started, marked paid, released or cancelled, with the deadline. |
| The fiat payment, the chat | no | |
| A dispute | partly | Who opened it and the ruling's amounts. Evidence is a fingerprint only. |

**An order on Keel's book**

| What | Posted | Details |
|---|---|---|
| The order and its fills | yes | Your account, the pair, the side, the price, the size, and each fill with its fee. |

</div>
</div>

## What an exchange posts about itself

This applies to both kinds of exchange. A person trading alone posts none
of it.

| What | Posted | Details |
|---|---|---|
| The client account | yes | Its address, and that governance registered it. |
| Public profile | yes | The name, the website and the logo link you set in Harbor (`clients.md`, "Your public profile"). |
| Retail fees | yes | Your fee on P2P releases, order-book fills and withdrawals. |
| What you earned | yes | Total retail fees per asset. |
| Plan | yes | The tier, when the period started and ends, the fee, the allowance left, and whether it renews. |
| Lock | yes | How much KEEL you hold locked. |
| Usage | yes | What you paid Keel, what you owe, and each charge with its kind. |
| Users | yes | How many accounts you vouch for, and which. |
| Service keys | yes | Each key's address, what it may sign, and when it expires. The key's secret is never posted and never leaves your backend. |
| Open offers under your name | yes | The count, and the offers themselves (non-custodial only). |

A custodial exchange also posts its vault:

| What | Posted | Details |
|---|---|---|
| Each vault | yes | The network, the vault's public key and chain code, and its generation. Public keys only: the signing key is yours and is never on Keel. |
| The signer's URL | yes | The address observers reach your signer at. Treat it as public and protect the signer accordingly. |
| Reserves against liabilities | yes | Per asset: what your vault holds on the external chain and what your users' balances add up to. Anyone can check that the first covers the second. |
| Rotations | yes | A pending rotation to a new key, and retired generations. |
| Deposit screening | yes | Whether deposits to your users land on hold. |

## What is never posted, on any path

- A person's name, email, phone number, address or date of birth.
- Identity documents, selfies, or anything else collected for KYC. A tier
  number is posted; what you checked to grant it is not.
- Passwords, wallet seed phrases and private keys.
- Bank account numbers, card numbers and payment references, unless
  someone types them into a public text field (an offer's terms or a
  memo). The apps warn against it; the chain cannot tell.
- The fiat payment itself.
- Chat messages and dispute evidence files.
- IP addresses, devices and browsing.
- A custodial exchange's internal offers, order book and matching.

## Three things to keep in mind

1. **It is permanent.** There is no way to remove or edit what a block
   holds. A correction is a new action, and the old one stays readable.
2. **Addresses link up.** An account's deposit addresses, withdrawals and
   trades are all listed under it. Someone who learns that one address is
   a given person can read that account's whole history.
3. **Free text is the only place personal data can leak in.** Keep offer
   terms and memos to what a stranger may read.

## Where to read it

| To read | Ask |
|---|---|
| One account | `GET /api/v1/accounts/{address}`, and its `/txs`, `/transfers`, `/offers`, `/trades`, `/orders` |
| Everything that happened to an address, in order | `GET /api/v1/events?address=` |
| Offers, all or one exchange's | `GET /api/v1/offers?client=` |
| Exchanges | `GET /api/v1/clients` |
| A custodial exchange's vaults and reserves | `GET /rpc/v1/custody/{address}` |
| The deposit addresses of a vault, with owners | `GET /rpc/v1/custody/{address}/{chain}/addresses`, `GET /rpc/v1/vaults/{chain}/addresses` |
| One action, in full | `GET /api/v1/txs/{id}` |

The same data is on the explorer, at
[testnet.keelchain.com](https://testnet.keelchain.com/). The fields of
each answer are in `explorer-api.md` and the node's API page.
