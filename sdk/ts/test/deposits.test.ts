// Deposit progress: parsing the public chain APIs and staging a deposit from
// "seen" to "credited".
import { test } from "node:test";
import assert from "node:assert/strict";
import { depositProgress, externalNetworks, externalTxUrl, isProcessing, parseMempoolTxs, parseTrc20, parseTrxTransfers, stageLabel } from "../src/deposits.ts";

const ADDR = "tb1q3nyvdyv9f44m5zmrduumvneynpmut0fxm68gn8";

test("mempool.space rows paying the address become incoming transactions with confirmations", () => {
  const rows = [
    { txid: "aa", status: { confirmed: true, block_height: 330_000, block_time: 1_794_000_000 }, vout: [{ scriptpubkey_address: ADDR, value: 100_000 }, { scriptpubkey_address: "other", value: 5 }] },
    { txid: "bb", status: { confirmed: false }, vout: [{ scriptpubkey_address: ADDR, value: 7 }] },
    { txid: "cc", status: { confirmed: true, block_height: 1 }, vout: [{ scriptpubkey_address: "other", value: 9 }] },
  ];
  const got = parseMempoolTxs(rows, ADDR, 330_001);
  assert.deepEqual(
    got.map((t) => [t.tx_hash, t.asset, t.amount, t.confirmations]),
    [
      ["aa", "BTC.BTC", "100000", 2],
      ["bb", "BTC.BTC", "7", 0],
    ],
  );
  assert.equal(parseMempoolTxs(rows, ADDR, null)[0]!.confirmations, null);
  assert.deepEqual(parseMempoolTxs("nonsense", ADDR, 1), []);
});

test("TronGrid rows: only USDT transfers to the address, TRX transfers, confirmations from the block height", () => {
  const to = "TYqumPzuwGPqBP4JiouDjr8ajD6g4ThPXY";
  const trc20 = {
    data: [
      { transaction_id: "d1", token_info: { symbol: "USDT", address: "TXYZ" }, block_timestamp: 1_790_937_819_000, to, type: "Transfer", value: "1000000000" },
      { transaction_id: "d2", token_info: { symbol: "USDC", address: "TABC" }, to, type: "Transfer", value: "5" },
      { transaction_id: "d3", token_info: { symbol: "USDT", address: "TFAKE" }, to, type: "Transfer", value: "5" },
      { transaction_id: "d4", token_info: { symbol: "USDT", address: "TXYZ" }, to: "someone-else", type: "Transfer", value: "5" },
    ],
  };
  const usdt = parseTrc20(trc20, to, "TXYZ");
  assert.deepEqual(usdt.map((t) => [t.tx_hash, t.asset, t.amount, t.time]), [["d1", "TRON.USDT", "1000000000", 1_790_937_819]]);
  assert.equal(parseTrc20(trc20, to).length, 2, "without a pinned contract both USDT-named tokens pass");
  const trx = { data: [{ txID: "t1", blockNumber: 100, block_timestamp: 5_000, raw_data: { contract: [{ type: "TransferContract", parameter: { value: { amount: 2_000_000 } } }] } }] };
  assert.deepEqual(parseTrxTransfers(trx, 118).map((t) => [t.tx_hash, t.asset, t.amount, t.confirmations]), [["t1", "TRON.TRX", "2000000", 19]]);
});

test("stages: seen, confirming, observing, then the chain's word wins", () => {
  const incoming = [
    { chain: "BTC" as const, tx_hash: "m", asset: "BTC.BTC", amount: "1", confirmations: 0, time: 3 },
    { chain: "BTC" as const, tx_hash: "c", asset: "BTC.BTC", amount: "2", confirmations: 1, time: 2 },
    { chain: "BTC" as const, tx_hash: "o", asset: "BTC.BTC", amount: "3", confirmations: 2, time: 1 },
    { chain: "BTC" as const, tx_hash: "k", asset: "BTC.BTC", amount: "4", confirmations: 9, time: 0 },
  ];
  const rows = [{ chain: "BTC", tx_hash: "k", asset: "BTC.BTC", amount: "4", status: "credited", votes: 3 }];
  const got = depositProgress(incoming, rows, 2);
  assert.deepEqual(
    got.map((p) => [p.tx_hash, p.stage]),
    [
      ["m", "seen"],
      ["c", "confirming"],
      ["o", "observing"],
      ["k", "credited"],
    ],
  );
  assert.equal(got[3]!.confirmations, 9, "the external lookup fills in confirmations on a chain row");
  assert.equal(stageLabel(got[1]!), "1 of 2 confirmations");
  assert.equal(isProcessing("observing"), true);
  assert.equal(isProcessing("credited"), false);
  // Pending chain rows without an external lookup are "observing" with their votes.
  const pending = depositProgress(null, [{ chain: "TRON", tx_hash: "p", asset: "TRON.USDT", amount: "1", status: "Pending", votes: 1 }], 19);
  assert.equal(pending[0]!.stage, "observing");
  assert.equal(stageLabel(pending[0]!), "Observers voting (1)");
});

test("the testnet bridges signet and Nile; links go to their explorers", () => {
  const t = externalNetworks("testnet");
  assert.equal(t.BTC.name, "Bitcoin signet");
  assert.equal(t.TRON.name, "Tron Nile");
  assert.equal(t.ETH.api, null);
  assert.equal(externalTxUrl(t.BTC, "ab"), "https://mempool.space/signet/tx/ab");
  assert.equal(externalTxUrl(t.TRON, "ab"), "https://nile.tronscan.org/#/transaction/ab");
  assert.equal(externalNetworks("mainnet").BTC.name, "Bitcoin");
});
