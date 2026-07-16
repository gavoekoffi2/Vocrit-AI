import "dotenv/config";
import express from "express";
import { nanoid } from "nanoid";
import { db, migrate } from "./db.js";
import { createCheckout, verifyWebhook, isStubMode } from "./moneyFusion.js";
import { issueLicense, verifyLicense, isAdmin } from "./licenses.js";

const {
  PORT = "3000",
  SUBSCRIPTION_PRICE_XOF = "10000",
  SUBSCRIPTION_CURRENCY = "XOF",
} = process.env;

migrate();

const app = express();
app.use(express.json({ limit: "128kb" }));

app.use((req, _res, next) => {
  console.log(`[${new Date().toISOString()}] ${req.method} ${req.path}`);
  next();
});

app.get("/api/health", (_req, res) => {
  res.json({ ok: true, now: new Date().toISOString() });
});

/**
 * Kick off a Money Fusion checkout. Called by the desktop app when a user
 * clicks "Subscribe" in the paywall modal.
 */
app.post("/api/checkout", async (req, res) => {
  try {
    const { email, phone } = req.body || {};
    const deviceId = req.body?.device_id || req.body?.deviceId || "";
    if (!email || !phone) {
      return res.status(400).json({ error: "email and phone are required" });
    }

    const normalisedEmail = email.toLowerCase().trim();

    // Admins get a free license immediately — no payment needed.
    if (isAdmin(normalisedEmail)) {
      const license = issueLicense({
        email: normalisedEmail,
        deviceId,
        tier: "admin_free",
      });
      return res.json({
        session_id: `admin_${nanoid(8)}`,
        checkout_url: null,
        status: "paid",
        email: normalisedEmail,
        license_key: license.key,
      });
    }

    const sessionId = nanoid(16);
    const amountXof = Number.parseInt(SUBSCRIPTION_PRICE_XOF, 10);

    db.prepare(
      `INSERT INTO checkout_sessions (id, email, phone, device_id, amount_xof, currency)
       VALUES (?, ?, ?, ?, ?, ?)`,
    ).run(
      sessionId,
      normalisedEmail,
      phone,
      deviceId || "",
      amountXof,
      SUBSCRIPTION_CURRENCY,
    );

    const { checkoutUrl, providerReference } = await createCheckout({
      sessionId,
      email: normalisedEmail,
      phone,
      amountXof,
      currency: SUBSCRIPTION_CURRENCY,
    });

    db.prepare(
      `UPDATE checkout_sessions
       SET checkout_url = ?, provider_reference = ?, updated_at = datetime('now')
       WHERE id = ?`,
    ).run(checkoutUrl, providerReference, sessionId);

    res.json({
      session_id: sessionId,
      checkout_url: checkoutUrl,
      status: "pending",
    });
  } catch (err) {
    console.error("[/api/checkout] failed", err);
    res.status(500).json({ error: String(err.message || err) });
  }
});

/**
 * Poll checkout status. The desktop app calls this after the user comes back
 * from the hosted Money Fusion page to know whether to activate the license.
 */
app.get("/api/checkout/:id", (req, res) => {
  const row = db
    .prepare("SELECT * FROM checkout_sessions WHERE id = ? LIMIT 1")
    .get(req.params.id);
  if (!row) return res.status(404).json({ error: "unknown_session" });

  let license = null;
  if (row.license_key) {
    license = db
      .prepare("SELECT * FROM licenses WHERE key = ? LIMIT 1")
      .get(row.license_key);
  }

  res.json({
    session_id: row.id,
    status: row.status,
    checkout_url: row.checkout_url,
    email: license?.email || row.email,
    license_key: license?.key || null,
    expires_at: license?.expires_at || null,
    tier: license?.tier || null,
  });
});

/**
 * Verify a license key from the desktop app. Called on startup and whenever
 * the user pastes a key into the settings UI.
 */
app.post("/api/license/verify", (req, res) => {
  const { key, email, deviceId } = req.body || {};
  res.json(verifyLicense({ key, email, deviceId }));
});

/**
 * Money Fusion webhook — mark a session as paid and issue a license.
 */
app.post("/api/webhook/money-fusion", (req, res) => {
  const providedSecret = req.headers["x-webhook-secret"] || req.body?.secret;
  if (!verifyWebhook(req.body, providedSecret)) {
    return res.status(401).json({ error: "bad_signature" });
  }

  const sessionId =
    req.body?.personal_Info?.[0]?.userId ||
    req.body?.sessionId ||
    req.body?.reference;
  const paid =
    req.body?.statut === true ||
    req.body?.status === "paid" ||
    req.body?.event === "payment.succeeded";

  if (!sessionId) {
    return res.status(400).json({ error: "missing_session_id" });
  }

  const session = db
    .prepare("SELECT * FROM checkout_sessions WHERE id = ? LIMIT 1")
    .get(sessionId);
  if (!session) return res.status(404).json({ error: "unknown_session" });

  if (!paid) {
    db.prepare(
      `UPDATE checkout_sessions
       SET status = 'failed', updated_at = datetime('now')
       WHERE id = ?`,
    ).run(sessionId);
    return res.json({ ok: true, status: "failed" });
  }

  const license = issueLicense({
    email: session.email,
    deviceId: session.device_id,
    tier: "premium",
  });

  db.prepare(
    `UPDATE checkout_sessions
     SET status = 'paid', license_key = ?, updated_at = datetime('now')
     WHERE id = ?`,
  ).run(license.key, sessionId);

  res.json({ ok: true, status: "paid", license });
});

/**
 * Dev-only stub that simulates a successful payment. The Money Fusion STUB
 * checkout URL points here so the full purchase flow can be exercised locally
 * without real credentials.
 *
 * Only registered in stub mode: in production this route would hand out free
 * premium licenses to anyone holding a session id.
 */
const escapeHtml = (value) =>
  String(value).replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );

if (isStubMode()) {
  app.all("/api/dev/fake-pay", (req, res) => {
    const sessionId = req.query.session || req.body?.session;
    if (!sessionId) return res.status(400).send("missing ?session=");
    const session = db
      .prepare("SELECT * FROM checkout_sessions WHERE id = ? LIMIT 1")
      .get(sessionId);
    if (!session) return res.status(404).send("unknown session");

    const license = issueLicense({
      email: session.email,
      deviceId: session.device_id,
      tier: "premium",
    });
    db.prepare(
      `UPDATE checkout_sessions
       SET status = 'paid', license_key = ?, updated_at = datetime('now')
       WHERE id = ?`,
    ).run(license.key, sessionId);

    res.type("html").send(
      `<h1>Vocrit AI — paiement simulé</h1>
         <p>Session <code>${escapeHtml(sessionId)}</code> marquée comme payée.</p>
         <p>Votre clé de licence&nbsp;: <code>${escapeHtml(license.key)}</code></p>
         <p>Vous pouvez fermer cette fenêtre et revenir à l'application.</p>`,
    );
  });
}

/**
 * Admin-only: issue a free lifetime license. Protected by ADMIN_MASTER_KEY
 * passed in the Authorization header ("Bearer <key>").
 */
app.post("/api/admin/grant", (req, res) => {
  const auth = (req.headers.authorization || "").replace(/^Bearer\s+/i, "");
  if (!auth || auth !== process.env.ADMIN_MASTER_KEY) {
    return res.status(401).json({ error: "unauthorised" });
  }
  const { email, deviceId } = req.body || {};
  if (!email) return res.status(400).json({ error: "email required" });
  const license = issueLicense({ email, deviceId, tier: "admin_free" });
  res.json({ license });
});

const port = Number.parseInt(PORT, 10);
app.listen(port, () => {
  console.log(`Vocrit AI licensing backend listening on :${port}`);
});
