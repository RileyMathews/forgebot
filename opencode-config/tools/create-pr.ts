import { tool } from "@opencode-ai/plugin"

export default tool({
  description: "Open a pull request on Forgejo. The body must contain 'Closes #<issue_id>' on its own line.",
  args: {
    title: tool.schema.string().describe("Pull request title"),
    body: tool.schema.string().describe("Pull request body (markdown). Must include 'Closes #N' on its own line."),
    head: tool.schema.string().describe("Source branch name (e.g. agent/issue-42)"),
    base: tool.schema.string().describe("Target branch name (e.g. main)"),
  },
  async execute(args) {
    const { FORGEBOT_FORGEJO_URL, FORGEBOT_FORGEJO_TOKEN, FORGEBOT_REPO } = process.env
    const res = await fetch(
      `${FORGEBOT_FORGEJO_URL}/api/v1/repos/${FORGEBOT_REPO}/pulls`,
      {
        method: "POST",
        headers: {
          "Authorization": `token ${FORGEBOT_FORGEJO_TOKEN}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ title: args.title, body: args.body, head: args.head, base: args.base }),
      }
    )
    return res.ok ? "Pull request created." : `Failed: ${res.status} ${await res.text()}`
  },
})
