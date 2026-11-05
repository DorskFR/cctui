# The worker image under test plus the probes and the T2 hook. Nothing else
# changes: the hook is inert unless a dispatch sets REDTEAM_RUN.
ARG WORKER_IMAGE
FROM ${WORKER_IMAGE}
COPY redteam/probes/ /opt/redteam/probes/
COPY redteam/ci/t2-sweep.sh /opt/redteam/t2-sweep.sh
COPY redteam/ci/t2-hook.sh /opt/worker-entrypoint.d/90-redteam-t2.sh
