-- Seeds the demo schema for the Oracle connector tests. Runs once, as SYS
-- against FREEPDB1, on the first start of an empty database — after the
-- image has created the APP_USER (zdbt) named in compose.yml.
--
-- The tables cover every branch of the type mapping: NUMBER with and
-- without a scale, VARCHAR2, BINARY_DOUBLE, native 23ai BOOLEAN, DATE,
-- TIMESTAMP, TIMESTAMP WITH TIME ZONE, CLOB and RAW, each with a NULL row
-- so the extractor's null handling is exercised too. DEMO_ORDERS carries a
-- primary key and an UPDATED_AT to drive incremental runs.

-- The image grants CONNECT/RESOURCE and a quota; these are the extras the
-- loader needs to publish a table by rename and to leave a view behind.
GRANT CREATE TABLE, CREATE VIEW, CREATE SEQUENCE TO zdbt;
ALTER USER zdbt QUOTA UNLIMITED ON USERS;

CREATE TABLE zdbt.demo_customers (
  id          NUMBER(10,0)   NOT NULL,
  name        VARCHAR2(100),
  country     CHAR(2),
  balance     NUMBER(18,2),
  score       BINARY_DOUBLE,
  active      BOOLEAN,
  signed_up   DATE,
  updated_at  TIMESTAMP(6),
  seen_at     TIMESTAMP(6) WITH TIME ZONE,
  notes       CLOB,
  avatar      RAW(16),
  CONSTRAINT demo_customers_pk PRIMARY KEY (id)
);

INSERT INTO zdbt.demo_customers VALUES (
  1, 'Ada Lovelace', 'GB', 1250.75, 0.91, TRUE,
  DATE '2024-03-01', TIMESTAMP '2026-01-01 10:00:00',
  TIMESTAMP '2026-01-01 10:00:00 +00:00', 'first customer',
  HEXTORAW('DEADBEEF'));
INSERT INTO zdbt.demo_customers VALUES (
  2, 'Grace Hopper', 'US', -40.00, 0.55, FALSE,
  DATE '2024-07-14', TIMESTAMP '2026-01-02 11:30:00',
  TIMESTAMP '2026-01-02 11:30:00 +02:00', NULL, NULL);
INSERT INTO zdbt.demo_customers VALUES (
  3, 'Alan Turing', NULL, NULL, NULL, NULL,
  NULL, TIMESTAMP '2026-01-03 12:00:00',
  NULL, 'no country, no balance', NULL);
INSERT INTO zdbt.demo_customers VALUES (
  4, 'Katherine Johnson', 'US', 9999.99, 1.0, TRUE,
  DATE '2025-11-30', TIMESTAMP '2026-01-04 09:15:00',
  TIMESTAMP '2026-01-04 09:15:00 -05:00', 'orbital mechanics', HEXTORAW('C0FFEE'));

CREATE TABLE zdbt.demo_orders (
  id           NUMBER(10,0)  NOT NULL,
  customer_id  NUMBER(10,0),
  reference    VARCHAR2(40),
  amount       NUMBER(18,2),
  placed_on    DATE,
  updated_at   TIMESTAMP(6),
  CONSTRAINT demo_orders_pk PRIMARY KEY (id)
);

INSERT INTO zdbt.demo_orders VALUES (
  1, 1, 'ORD-0001', 100.00, DATE '2026-01-01', TIMESTAMP '2026-01-01 10:05:00');
INSERT INTO zdbt.demo_orders VALUES (
  2, 1, 'ORD-0002', 250.50, DATE '2026-01-02', TIMESTAMP '2026-01-02 10:05:00');
INSERT INTO zdbt.demo_orders VALUES (
  3, 2, 'ORD-0003', 75.25, DATE '2026-01-03', TIMESTAMP '2026-01-03 10:05:00');
INSERT INTO zdbt.demo_orders VALUES (
  4, 4, 'ORD-0004', NULL, NULL, TIMESTAMP '2026-01-04 10:05:00');

-- Views are listed by the table explorer too, so keep one around.
CREATE VIEW zdbt.demo_active_customers AS
  SELECT id, name, country, balance FROM zdbt.demo_customers WHERE active = TRUE;

COMMIT;
