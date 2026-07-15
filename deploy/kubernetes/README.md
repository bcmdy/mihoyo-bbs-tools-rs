# Kubernetes CronJob 部署

本目录的 `cronjob.yaml` 按北京时间每日 `00:05` 执行一次。任务由 Kubernetes 调度，不要在配置中同时启用 `runtime.schedule`。

先复制并编辑完整配置，建议把 `runtime.schedule.enabled` 设为 `false`。配置包含 Cookie、SToken 和通知凭据，只能保存到 Kubernetes Secret，不能放入 ConfigMap 或提交到 Git：

```bash
kubectl create secret generic mihoyo-bbs-tools-config \
  --from-file=config.yaml=./config/config.yaml \
  --dry-run=client -o yaml | kubectl apply -f -
```

正式部署前，将 `cronjob.yaml` 的镜像从 `latest` 改为已发布的固定版本标签或镜像摘要，然后应用清单：

```bash
kubectl apply -f deploy/kubernetes/cronjob.yaml
```

配置卷不可写，因此任务使用 `--read-only`。自动刷新的凭据可完成当次任务，但不会持久化回 Secret；需要保存时应在集群外安全更新配置并重新创建 Secret。

查看状态和日志：

```bash
kubectl get cronjob mihoyo-bbs-tools-rs
kubectl get jobs --selector=app.kubernetes.io/name=mihoyo-bbs-tools-rs
kubectl logs job/<任务名称>
```

手动创建一次性任务：

```bash
kubectl create job --from=cronjob/mihoyo-bbs-tools-rs mihoyo-bbs-tools-manual
kubectl logs -f job/mihoyo-bbs-tools-manual
```

清单要求支持 CronJob `timeZone` 的 Kubernetes 1.27 或更高版本。旧集群应删除 `timeZone` 并按控制平面时区换算 `schedule`；UTC 集群可使用 `5 16 * * *`。任务使用 `Forbid` 并发策略且不进行整 Job 重试，避免对全部账号重复请求。文件日志写入临时 `emptyDir`，Job 删除后不会保留；长期日志应交给集群日志系统收集。
