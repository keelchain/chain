-- Testnet faucet claims (2026-10-02): one row per asset sent, keyed so the
-- cooldown per address and the daily cap per IP are plain queries.
CREATE TABLE IF NOT EXISTS faucet_claims (
    id          BIGSERIAL PRIMARY KEY,
    address     TEXT NOT NULL,
    ip          TEXT NOT NULL,
    asset       TEXT NOT NULL,
    amount      NUMERIC(40,0) NOT NULL,
    tx_id       TEXT NOT NULL,
    claimed_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS faucet_claims_address ON faucet_claims (address, claimed_at DESC);
CREATE INDEX IF NOT EXISTS faucet_claims_ip ON faucet_claims (ip, claimed_at DESC);
