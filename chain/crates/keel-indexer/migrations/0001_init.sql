-- Keel indexer schema. Amounts are NUMERIC(40,0) (u128 fits); timestamps are
-- block times in milliseconds; addresses / hashes are lowercase hex.
-- Every table is keyed so a re-run of the same block is a no-op.

CREATE TABLE sync_state (
    network              TEXT PRIMARY KEY,
    chain_id             BIGINT,
    indexed_height       BIGINT NOT NULL DEFAULT 0,
    first_indexed_height BIGINT,
    last_state_hash      TEXT,
    hash_mismatch_height BIGINT,
    hash_mismatch_node   TEXT,
    hash_mismatch_stored TEXT,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE blocks (
    height          BIGINT PRIMARY KEY,
    timestamp       BIGINT NOT NULL,
    -- false when the node served no timestamp for this height (empty block
    -- through the receipts fallback) and it was carried forward.
    timestamp_exact BOOLEAN NOT NULL DEFAULT true,
    state_hash      TEXT,
    tx_count        INTEGER NOT NULL DEFAULT 0,
    ok_count        INTEGER NOT NULL DEFAULT 0,
    event_count     INTEGER NOT NULL DEFAULT 0,
    proposer        TEXT
);
CREATE INDEX blocks_timestamp ON blocks (timestamp);

-- One row per receipt. The same tx_id can land in two blocks (duplicate
-- inclusion, the later one fails BAD_NONCE), so the key is (height, index).
CREATE TABLE txs (
    height        BIGINT NOT NULL,
    index         INTEGER NOT NULL,
    tx_id         TEXT NOT NULL,
    timestamp     BIGINT NOT NULL,
    signer        TEXT NOT NULL,
    nonce         BIGINT,
    module        TEXT NOT NULL,
    kind          TEXT NOT NULL,
    action        JSONB,
    ok            BOOLEAN NOT NULL,
    error_code    TEXT,
    error_message TEXT,
    event_count   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (height, index)
);
CREATE INDEX txs_tx_id ON txs (tx_id);
CREATE INDEX txs_signer ON txs (signer, height DESC, index DESC);
CREATE INDEX txs_module ON txs (module, height DESC, index DESC);
CREATE INDEX txs_ok ON txs (ok, height DESC, index DESC);

-- Flattened events; tx_index = -1 for block-level (end-of-block) events.
CREATE TABLE events (
    height      BIGINT NOT NULL,
    tx_index    INTEGER NOT NULL,
    event_index INTEGER NOT NULL,
    tx_id       TEXT,
    timestamp   BIGINT NOT NULL,
    type        TEXT NOT NULL,
    data        JSONB NOT NULL,
    addresses   TEXT[] NOT NULL DEFAULT '{}',
    PRIMARY KEY (height, tx_index, event_index)
);
CREATE INDEX events_type ON events (type, height DESC);
CREATE INDEX events_addresses ON events USING GIN (addresses);
CREATE INDEX events_tx_id ON events (tx_id);

CREATE TABLE account_stats (
    address           TEXT PRIMARY KEY,
    first_seen_height BIGINT NOT NULL,
    last_seen_height  BIGINT NOT NULL,
    tx_count          BIGINT NOT NULL DEFAULT 0
);

-- Materialized value movements (Transferred, DepositCredited,
-- OutboundConfirmed, OrderFilled legs, TradeReleased).
CREATE TABLE transfers (
    height      BIGINT NOT NULL,
    tx_index    INTEGER NOT NULL,
    event_index INTEGER NOT NULL,
    leg         SMALLINT NOT NULL DEFAULT 0,
    tx_id       TEXT,
    timestamp   BIGINT NOT NULL,
    asset       TEXT NOT NULL,
    amount      NUMERIC(40,0) NOT NULL,
    from_addr   TEXT,
    to_addr     TEXT,
    kind        TEXT NOT NULL,
    PRIMARY KEY (height, tx_index, event_index, leg)
);
CREATE INDEX transfers_from ON transfers (from_addr, height DESC, tx_index DESC, event_index DESC, leg DESC);
CREATE INDEX transfers_to ON transfers (to_addr, height DESC, tx_index DESC, event_index DESC, leg DESC);
CREATE INDEX transfers_asset_ts ON transfers (asset, timestamp DESC);

CREATE TABLE fills (
    height         BIGINT NOT NULL,
    tx_index       INTEGER NOT NULL,
    event_index    INTEGER NOT NULL,
    tx_id          TEXT,
    timestamp      BIGINT NOT NULL,
    pair           TEXT NOT NULL,
    price          NUMERIC(40,0) NOT NULL,
    quantity       NUMERIC(40,0) NOT NULL,
    quote          NUMERIC(40,0) NOT NULL,
    fee            NUMERIC(40,0) NOT NULL DEFAULT 0,
    fee_asset      TEXT,
    taker          TEXT,
    taker_side     TEXT,
    taker_order_id BIGINT NOT NULL,
    maker_order_id BIGINT,
    maker          TEXT,
    PRIMARY KEY (height, tx_index, event_index)
);
CREATE INDEX fills_pair ON fills (pair, height DESC, tx_index DESC, event_index DESC);
CREATE INDEX fills_pair_ts ON fills (pair, timestamp DESC);
CREATE INDEX fills_taker_order ON fills (taker_order_id);
CREATE INDEX fills_maker_order ON fills (maker_order_id);

-- 1-minute OHLCV rollups computed on insert; 5m/1h/1d aggregate on read.
CREATE TABLE candles (
    pair   TEXT NOT NULL,
    bucket BIGINT NOT NULL,          -- minute start, ms
    o      NUMERIC(40,0) NOT NULL,
    h      NUMERIC(40,0) NOT NULL,
    l      NUMERIC(40,0) NOT NULL,
    c      NUMERIC(40,0) NOT NULL,
    v      NUMERIC(40,0) NOT NULL,   -- base volume
    qv     NUMERIC(40,0) NOT NULL,   -- quote volume
    n      INTEGER NOT NULL,
    PRIMARY KEY (pair, bucket)
);

CREATE TABLE orders (
    id             BIGINT PRIMARY KEY,
    owner          TEXT NOT NULL,
    pair           TEXT NOT NULL,
    side           TEXT,
    order_type     TEXT,
    price          NUMERIC(40,0),
    quantity       NUMERIC(40,0),
    quote_budget   NUMERIC(40,0),
    client_id      BIGINT,
    resting        NUMERIC(40,0),
    filled         NUMERIC(40,0) NOT NULL DEFAULT 0,
    filled_quote   NUMERIC(40,0) NOT NULL DEFAULT 0,
    released       NUMERIC(40,0),
    status         TEXT NOT NULL,
    created_height BIGINT NOT NULL,
    updated_height BIGINT NOT NULL,
    tx_id          TEXT
);
CREATE INDEX orders_owner ON orders (owner, id DESC);
CREATE INDEX orders_pair ON orders (pair, id DESC);

CREATE TABLE offers (
    id                  BIGINT PRIMARY KEY,
    owner               TEXT NOT NULL,
    side                TEXT,
    asset               TEXT,
    fiat_currency       TEXT,
    payment_method      TEXT,
    margin_bps          INTEGER,
    fixed_price         NUMERIC(40,0),
    min_amount          NUMERIC(40,0),
    max_amount          NUMERIC(40,0),
    payment_window_secs INTEGER,
    country             TEXT,
    min_tier            INTEGER,
    status              TEXT NOT NULL,
    created_height      BIGINT NOT NULL,
    updated_height      BIGINT NOT NULL,
    tx_id               TEXT
);
CREATE INDEX offers_owner ON offers (owner, id DESC);
CREATE INDEX offers_filter ON offers (asset, side, status, id DESC);

CREATE TABLE trades (
    id             BIGINT PRIMARY KEY,
    offer_id       BIGINT,
    buyer          TEXT,
    seller         TEXT,
    asset          TEXT,
    amount         NUMERIC(40,0),
    fee            NUMERIC(40,0),
    fiat_amount    NUMERIC(40,0),
    fiat_currency  TEXT,
    status         TEXT NOT NULL,
    started_height BIGINT,
    started_at     BIGINT,
    deadline       BIGINT,
    paid_at        BIGINT,
    closed_height  BIGINT,
    updated_height BIGINT NOT NULL,
    tx_id          TEXT,
    dispute        JSONB,
    history        JSONB NOT NULL DEFAULT '[]'::jsonb
);
CREATE INDEX trades_offer ON trades (offer_id, id DESC);
CREATE INDEX trades_buyer ON trades (buyer, id DESC);
CREATE INDEX trades_seller ON trades (seller, id DESC);

CREATE TABLE deposits (
    key             TEXT PRIMARY KEY,
    chain           TEXT,
    asset           TEXT,
    owner           TEXT,
    amount          NUMERIC(40,0),
    status          TEXT NOT NULL,
    tx_hash         TEXT,
    external_index  INTEGER,
    deposit_index   BIGINT,
    external_height BIGINT,
    votes           INTEGER,
    height          BIGINT NOT NULL,
    tx_id           TEXT,
    release_height  BIGINT,
    updated_height  BIGINT NOT NULL
);
CREATE INDEX deposits_owner ON deposits (owner, height DESC);
CREATE INDEX deposits_chain ON deposits (chain, status, height DESC);

CREATE TABLE outbounds (
    id               BIGINT PRIMARY KEY,
    owner            TEXT,
    asset            TEXT,
    chain            TEXT,
    to_addr          TEXT,
    amount           NUMERIC(40,0),
    fee_asset        TEXT,
    fee_estimate     NUMERIC(40,0),
    status           TEXT NOT NULL,
    batch_id         BIGINT,
    tx_hash          TEXT,
    refunded         NUMERIC(40,0),
    created_height   BIGINT,
    confirmed_height BIGINT,
    updated_height   BIGINT NOT NULL,
    tx_id            TEXT
);
CREATE INDEX outbounds_owner ON outbounds (owner, id DESC);
CREATE INDEX outbounds_status ON outbounds (status, id DESC);

CREATE TABLE proposals (
    id             BIGINT PRIMARY KEY,
    proposer       TEXT,
    title          TEXT,
    description    TEXT,
    kind           JSONB,
    status         TEXT NOT NULL,
    deposit        NUMERIC(40,0),
    submit_height  BIGINT,
    voting_end     BIGINT,
    timelock_end   BIGINT,
    yes            NUMERIC(40,0) NOT NULL DEFAULT 0,
    no             NUMERIC(40,0) NOT NULL DEFAULT 0,
    abstain        NUMERIC(40,0) NOT NULL DEFAULT 0,
    veto           NUMERIC(40,0) NOT NULL DEFAULT 0,
    executed_ok    BOOLEAN,
    updated_height BIGINT NOT NULL,
    tx_id          TEXT
);

CREATE TABLE votes (
    proposal_id BIGINT NOT NULL,
    voter       TEXT NOT NULL,
    choice      TEXT,
    weight      NUMERIC(40,0),
    height      BIGINT NOT NULL,
    tx_id       TEXT,
    PRIMARY KEY (proposal_id, voter)
);

CREATE TABLE param_history (
    height      BIGINT NOT NULL,
    tx_index    INTEGER NOT NULL,
    event_index INTEGER NOT NULL,
    timestamp   BIGINT NOT NULL,
    key         TEXT NOT NULL,
    from_value  NUMERIC(40,0),
    to_value    NUMERIC(40,0) NOT NULL,
    tx_id       TEXT,
    PRIMARY KEY (height, tx_index, event_index)
);
CREATE INDEX param_history_key ON param_history (key, height DESC);

-- Snapshot from the node (start, every EpochAdvanced, periodic).
CREATE TABLE validators (
    address        TEXT PRIMARY KEY,
    consensus_key  TEXT,
    self_bond      NUMERIC(40,0) NOT NULL DEFAULT 0,
    delegated      NUMERIC(40,0) NOT NULL DEFAULT 0,
    power          NUMERIC(40,0) NOT NULL DEFAULT 0,
    jailed         BOOLEAN NOT NULL DEFAULT false,
    joined_epoch   BIGINT,
    in_consensus   BOOLEAN NOT NULL DEFAULT false,
    epoch          BIGINT,
    updated_height BIGINT NOT NULL,
    updated_at     BIGINT NOT NULL
);

CREATE TABLE epochs (
    epoch           BIGINT PRIMARY KEY,
    start_height    BIGINT NOT NULL,
    validator_count INTEGER,
    validators      JSONB NOT NULL DEFAULT '[]'::jsonb
);

-- Node snapshots used by the read side.
CREATE TABLE markets (
    pair           TEXT PRIMARY KEY,
    base           TEXT NOT NULL,
    quote          TEXT NOT NULL,
    base_decimals  INTEGER NOT NULL,
    quote_decimals INTEGER NOT NULL,
    cfg            JSONB,
    last_price     NUMERIC(40,0),
    updated_at     BIGINT NOT NULL
);

CREATE TABLE assets (
    asset      TEXT PRIMARY KEY,
    decimals   INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    chain      TEXT,
    supply     NUMERIC(40,0) NOT NULL DEFAULT 0,
    holders    BIGINT NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL DEFAULT 0
);

-- Balances reconciled from the node's /v1/accounts/{addr} for every address
-- the indexer has seen (supply and holder counts derive from here).
CREATE TABLE balances (
    address      TEXT NOT NULL,
    asset        TEXT NOT NULL,
    account_type TEXT NOT NULL,
    balance      NUMERIC(40,0) NOT NULL,
    updated_at   BIGINT NOT NULL,
    PRIMARY KEY (address, asset, account_type)
);
CREATE INDEX balances_asset ON balances (asset, balance DESC);
