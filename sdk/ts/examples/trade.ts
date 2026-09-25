// Place a crossing pair of limit orders on a devnet and print the fills.
import { Keypair, RpcClient } from "../src/index.ts";

const rpc = new RpcClient(process.env.KEEL_RPC ?? "http://127.0.0.1:5000");
const alice = Keypair.fromSeed(0);
const bob = Keypair.fromSeed(1);
const price = 60_000n * 1_000_000n;
const sell = await rpc.send(alice, { PlaceOrder: { pair: "BTC-KUSD", side: "sell", order_type: "limit", price, quantity: 100_000_000n } });
console.log("sell", (await rpc.waitReceipt(sell)).ok);
const buy = await rpc.send(bob, { PlaceOrder: { pair: "BTC-KUSD", side: "buy", order_type: "limit", price, quantity: 100_000_000n } });
console.log("buy", JSON.stringify((await rpc.waitReceipt(buy)).events));
