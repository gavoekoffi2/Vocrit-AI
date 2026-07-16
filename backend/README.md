# Vocrit AI — licensing & payment backend

Tiny Node.js/Express service that powers the Vocrit AI subscription flow.

- Creates Money Fusion hosted checkouts (mobile money — Wave, Orange Money,
  MTN MoMo, Moov Money…) for the 10 000 XOF / year plan.
- Receives Money Fusion webhooks and issues license keys.
- Verifies license keys for the desktop app.
- Grants permanent "AdminFree" licenses to a configurable list of admin emails
  and via a master key baked into the desktop binary.

## Quick start

```bash
cd backend
cp .env.example .env   # fill in Money Fusion credentials + admin key
npm install
npm run migrate
npm run dev            # or: npm start
```

By default the service listens on `http://localhost:3000` and stores data in
`backend/vocrit.db` (SQLite).

If `MONEY_FUSION_API_KEY` is not set **and** `NODE_ENV` is not `production`,
the service runs in **stub mode**: the checkout URL points to
`/api/dev/fake-pay` which simulates a successful payment — useful for testing
the desktop app end-to-end without a real merchant account. The
`/api/dev/fake-pay` route is only registered in stub mode; it never exists in
production.

## Security requirements (production)

- `MONEY_FUSION_WEBHOOK_SECRET` **must** be set in production. Webhooks are
  rejected when it is missing — an unsigned webhook endpoint would let anyone
  mark sessions as paid and mint licenses.
- `ADMIN_MASTER_KEY` is optional; when unset, both `/api/admin/grant` and the
  desktop offline admin-key path are disabled (fail closed).
- The desktop app now verifies every license key against
  `POST /api/license/verify` before activating it — keys are never trusted
  client-side.

## Endpoints

| Method | Path                        | Description                                                |
| ------ | --------------------------- | ---------------------------------------------------------- |
| GET    | `/api/health`               | Liveness probe                                             |
| POST   | `/api/checkout`             | Create a checkout session; returns a hosted payment URL    |
| GET    | `/api/checkout/:id`         | Poll the status of a checkout session                      |
| POST   | `/api/license/verify`       | Verify a license key (called by the desktop app)           |
| POST   | `/api/webhook/money-fusion` | Money Fusion → backend webhook (issues the license)        |
| POST   | `/api/admin/grant`          | Admin-only: issue a free lifetime license for a given user |
| ALL    | `/api/dev/fake-pay`         | Dev-only payment simulator (stub mode)                     |

### `POST /api/checkout`

```json
{ "email": "user@example.com", "phone": "+22507XXXXXXXX", "deviceId": "..." }
```

Returns:

```json
{
  "sessionId": "cN1pTq8...",
  "checkoutUrl": "https://pay.moneyfusion.net/...",
  "status": "pending"
}
```

For admin emails the response is instant and contains the license:

```json
{
  "sessionId": "admin_...",
  "status": "paid",
  "license": { "key": "VOCRIT-..." }
}
```

### `POST /api/admin/grant`

```
Authorization: Bearer <ADMIN_MASTER_KEY>
Content-Type: application/json

{ "email": "friend@example.com" }
```

Hands out a free lifetime (AdminFree) license — the user just has to paste
the returned key into **Settings → Subscription** in the app.

## Deployment notes

- Host anywhere that can run Node 20+ (Railway, Fly.io, a VPS…).
- Put it behind HTTPS (Money Fusion will refuse to POST webhooks to an
  insecure origin in production).
- Mount a volume at `DATABASE_PATH` — the SQLite file is the source of truth
  for licenses.
- Set the same `ADMIN_MASTER_KEY` here and as the `VOCRIT_ADMIN_MASTER_KEY`
  build-time environment variable for the desktop app. This lets admins
  unlock the app offline using just the master key.

## Switching payment provider

`src/moneyFusion.js` exposes two functions (`createCheckout`,
`verifyWebhook`). Re-implement them against CinetPay, PayDunya or
Flutterwave to migrate without touching the rest of the server.
