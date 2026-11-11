# vendor/charts-rs 说明

本目录是 roe 自己维护的 charts-rs 补丁版,作为普通源码随 roe 一起提交,不再依赖 fork。

- 上游:https://github.com/vicanso/charts-rs.git
- 上游基准提交:`241667b5c2b8e34bf36a4ba375b31fe34e195893`(chore: version 2.3.0)
- 我方补丁:9 个 ROE-PATCH 提交,13 个文件,190 行增 / 33 行删;代码里带 `ROE-PATCH` 标记。默认行为与上游一致。
- 并入日期:2026-11-11。并入前最后一个提交:335a95f(原 fork roe-office/charts-rs 分支 roe-2.3,仅作备份,不再使用)。
- 完整旧历史:家目录 `charts-rs_dotgit_20261111`(原 .git)、`charts-rs_all_20261111.bundle` 等备份。
- `ROE-PATCHES.diff`:并入当天的补丁快照(相对基准 241667b)。之后继续改动,所以升级前必须按下面的步骤重新生成,不要直接用快照。
- 必须保留 LICENSE。

## 升级到新版上游的步骤

1. 取上游完整历史并回到旧基准:
   `git clone https://github.com/vicanso/charts-rs.git /tmp/cr_up && cd /tmp/cr_up && git checkout 241667b`
2. 把当前 vendor 内容覆盖进去,生成我方补丁(带 index 行,可 3 方合并):
   `rsync -a --delete --exclude=.git --exclude=ROE_VENDOR.md --exclude=ROE-PATCHES.diff ~/Projects/roe/vendor/charts-rs/ ./`
   `git diff > /tmp/ours.diff`
3. 丢弃工作区改动,切到新上游版本,套补丁:
   `git checkout -f <新版本>` 然后 `git apply --3way /tmp/ours.diff`,在冲突处手工解决(冲突只会出现在带 ROE-PATCH 的文件里)。
4. 把结果同步回 roe(保留本说明和补丁快照):
   `rsync -a --delete --exclude=.git --exclude=ROE_VENDOR.md --exclude=ROE-PATCHES.diff ./ ~/Projects/roe/vendor/charts-rs/`
5. 更新本文件的"上游基准提交"和日期,重新生成 `ROE-PATCHES.diff`,在 roe 里 `cargo check -p roe-core -p roe-impress -p roe-api -p roe-writer`,再跑图表相关测试。
