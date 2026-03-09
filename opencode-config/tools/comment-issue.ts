import { tool } from "@opencode-ai/plugin"

function requiredEnv(name: string): string {
  const value = process.env[name]
  if (!value || value.trim() === "") {
    throw new Error(`Missing required environment variable: ${name}`)
  }
  return value.trim()
}

function normalizeForgejoBaseUrl(url: string): string {
  return url.replace(/\/+$/, "")
}

function normalizeRepoFullName(value: string): string {
  const repo = value.trim()

  // Accept owner/repo directly.
  if (/^[^/]+\/[^/]+$/.test(repo)) {
    return repo
  }

  // Accept full repo URL and extract owner/repo.
  if (repo.startsWith("http://") || repo.startsWith("https://")) {
    const parsed = new URL(repo)
    const parts = parsed.pathname.split("/").filter(Boolean)
    if (parts.length >= 2) {
      return `${parts[0]}/${parts[1].replace(/\.git$/, "")}`
    }
  }

  throw new Error(
    `Invalid FORGEBOT_REPO='${repo}'. Expected 'owner/repo' or full repo URL.`
  )
}

export default tool({
  description: "Post a markdown comment on a Forgejo issue",
  args: {
    body: tool.schema.string().describe("Markdown content of the comment"),
  },
  async execute(args) {
    const forgejoUrl = normalizeForgejoBaseUrl(requiredEnv("FORGEBOT_FORGEJO_URL"))
    const token = requiredEnv("FORGEBOT_FORGEJO_TOKEN")
    const repo = normalizeRepoFullName(requiredEnv("FORGEBOT_REPO"))
    const issueId = requiredEnv("FORGEBOT_ISSUE_ID")
    const res = await fetch(
      `${forgejoUrl}/api/v1/repos/${repo}/issues/${issueId}/comments`,
      {
        method: "POST",
        headers: {
          "Authorization": `token ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ body: args.body }),
      }
    )
    return res.ok ? "Comment posted." : `Failed: ${res.status} ${await res.text()}`
  },
})
