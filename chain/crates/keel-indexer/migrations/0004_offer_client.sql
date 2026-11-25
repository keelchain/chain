-- The client an offer is listed under, as the chain stamped it when the
-- offer was created (`OfferCreated.client`).
ALTER TABLE offers ADD COLUMN client TEXT;
CREATE INDEX offers_client ON offers (client, id DESC) WHERE client IS NOT NULL;
