-- The native transaction id when the chain's 32-byte `tx_hash` is only its
-- hash (Solana: the base58 signature, schema 7 of the node).
ALTER TABLE deposits ADD COLUMN external_tx TEXT;
ALTER TABLE outbounds ADD COLUMN external_tx TEXT;
