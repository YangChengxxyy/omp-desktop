// Test fixture for omp-desktop's headless UI tests: `/dialogs` opens a
// confirm, an input, and an editor dialog in turn (no model call), then
// reports the answers through `notify`.
export default function (pi: any) {
	pi.registerCommand("dialogs", {
		description: "omp-desktop test: confirm, input, editor",
		handler: async (_args: string, ctx: any) => {
			const confirmed = await ctx.ui.confirm("确认标题", "继续吗？");
			const name = await ctx.ui.input("名字", "占位");
			const text = await ctx.ui.editor("编辑", "预填");
			ctx.ui.notify(`dialogs confirmed=${confirmed} name=${name} text=${text}`, "info");
		},
	});
}
