import { tool } from "@opencode-ai/plugin"

export default tool({
  description: "Post a markdown comment on a Forgejo issue",
  args: {
    body: tool.schema.string().describe("Markdown content of the comment"),
  },
  async execute(args) {
    const { FORGEBOT_FORGEJO_URL, FORGEBOT_FORGEJO_TOKEN, FORGEBOT_REPO, FORGEBOT_ISSUE_ID } = process.env
    const res = await fetch(
      `${FORGEBOT_FORGEJO_URL}/api/v1/repos/${FORGEBOT_REPO}/issues/${FORGEBOT_ISSUE_ID}/comments`,
      {
        method: "POST",
        headers: {
          "Authorization": `token ${FORGEBOT_FORGEJO_TOKEN}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ body: args.body }),
      }
    )
    return res.ok ? "Comment posted." : `Failed: ${res.status} ${await res.text()}`
  },
})
