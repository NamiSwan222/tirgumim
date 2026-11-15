// Email. No provider is chosen yet, so the sign-in link is written to the log. A real provider (Resend, Postmark,
// Amazon SES, ...) is an object with the same `sendSignIn` method.

export function logMailer(log = console.log) {
  return {
    configured: false,
    async sendSignIn({ to, link, code }) {
      log(`[mail] no mail provider configured. Sign-in link for ${to} (code ${code}): ${link}`)
    },
  }
}
