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

  if (/^[^/]+\/[^/]+$/.test(repo)) {
    return repo
  }

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
  description: "Open a pull request on Forgejo. The body must contain 'Closes #<issue_id>' on its own line.",
  args: {
    title: tool.schema.string().describe("Pull request title"),
    body: tool.schema.string().describe("Pull request body (markdown). Must include 'Closes #N' on its own line."),
    head: tool.schema.string().describe("Source branch name (e.g. agent/issue-42)"),
    base: tool.schema.string().describe("Target branch name (e.g. main)"),
  },
  async execute(args) {
    const forgejoUrl = normalizeForgejoBaseUrl(requiredEnv("FORGEBOT_FORGEJO_URL"))
    const token = requiredEnv("FORGEBOT_FORGEJO_TOKEN")
    const repo = normalizeRepoFullName(requiredEnv("FORGEBOT_REPO"))
    const res = await fetch(
      `${forgejoUrl}/api/v1/repos/${repo}/pulls`,
      {
        method: "POST",
        headers: {
          "Authorization": `token ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ title: args.title, body: args.body, head: args.head, base: args.base }),
      }
    )
    return res.ok ? "Pull request created." : `Failed: ${res.status} ${await res.text()}`
  },
})
