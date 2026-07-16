/**
 * Money Fusion integration.
 *
 * Money Fusion is an African mobile-money payment aggregator
 * (Wave, Orange Money, MTN MoMo, Moov Money, …). Integration documentation:
 *   https://pay.moneyfusion.net/
 *
 * The public FusionPay examples create a payment with fields such as
 * `total_price`, `articles`, `numero_send`, `nom_client`, `user_id`,
 * `order_id`, and `return_url`, then return a token and hosted payment URL.
 * Money Fusion merchant dashboards may expose account-specific API URLs, so
 * keep the URL configurable before going live.
 *
 * This module is intentionally small so it can be swapped for another
 * aggregator (CinetPay, PayDunya, Flutterwave) by re-implementing the same
 * two functions.
 */

const {
  MONEY_FUSION_API_URL = "https://www.pay.moneyfusion.net/api/checkout",
  MONEY_FUSION_API_KEY,
  PUBLIC_BASE_URL = "http://localhost:3000",
  CHECKOUT_SUCCESS_URL = "https://vocrit.ai/thank-you",
  CHECKOUT_CANCEL_URL = "https://vocrit.ai/payment-cancelled",
} = process.env;

export async function createCheckout({
  sessionId,
  email,
  phone,
  amountXof,
  currency,
}) {
  if (!MONEY_FUSION_API_KEY) {
    // Scaffold mode — return a fake hosted URL so the desktop app can be
    // end-to-end tested without a real merchant account. Replace with the
    // real API call below as soon as credentials are available.
    console.warn(
      "[moneyFusion] MONEY_FUSION_API_KEY is not set — using STUB checkout URL",
    );
    return {
      checkoutUrl: `${PUBLIC_BASE_URL}/api/dev/fake-pay?session=${sessionId}`,
      providerReference: `stub_${sessionId}`,
    };
  }

  const body = {
    total_price: String(amountXof),
    currency,
    articles: [
      {
        name: "Vocrit AI annual subscription",
        price: String(amountXof),
        quantity: 1,
      },
    ],
    numero_send: phone,
    nom_client: email,
    user_id: sessionId,
    order_id: sessionId,
    return_url: `${CHECKOUT_SUCCESS_URL}?session=${sessionId}`,
    cancel_url: CHECKOUT_CANCEL_URL,
    webhook_url: `${PUBLIC_BASE_URL}/api/webhook/money-fusion`,
  };

  const res = await fetch(MONEY_FUSION_API_URL, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${MONEY_FUSION_API_KEY}`,
    },
    body: JSON.stringify(body),
  });

  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Money Fusion checkout failed (${res.status}): ${text}`);
  }

  const data = await res.json();
  // The exact response shape depends on the Money Fusion version — adjust if
  // their docs change. Typical shape: { statut: true, url: "...", token: "..." }
  const checkoutUrl = data.url || data.checkout_url || data.payment_url;
  const providerReference = data.token || data.reference || sessionId;
  if (!checkoutUrl) {
    throw new Error(
      "Money Fusion response did not contain a checkout URL: " +
        JSON.stringify(data),
    );
  }
  return { checkoutUrl, providerReference };
}

/** True when running without real Money Fusion credentials (local testing). */
export function isStubMode() {
  return !MONEY_FUSION_API_KEY && process.env.NODE_ENV !== "production";
}

/**
 * Verify that a webhook body came from Money Fusion. The real verification
 * should check an HMAC signature in the request header against the secret
 * configured in the merchant dashboard — their docs explain the scheme.
 *
 * Fails CLOSED when no secret is configured outside stub mode: an unsigned
 * webhook endpoint would let anyone mark sessions as paid and mint licenses.
 */
export function verifyWebhook(body, providedSecret) {
  const secret = process.env.MONEY_FUSION_WEBHOOK_SECRET;
  if (!secret) {
    if (isStubMode()) return true; // local dev only
    console.error(
      "[moneyFusion] MONEY_FUSION_WEBHOOK_SECRET is not set — rejecting webhook",
    );
    return false;
  }
  return providedSecret === secret;
}
