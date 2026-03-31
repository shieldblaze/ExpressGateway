/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.controlplane.grpc.server;

import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import com.shieldblaze.expressgateway.controlplane.v1.NodeStatsReport;
import com.shieldblaze.expressgateway.controlplane.v1.StatsAck;
import com.shieldblaze.expressgateway.controlplane.v1.StatsCollectionServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.Timestamp;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import lombok.extern.log4j.Log4j2;

import java.util.Objects;

/**
 * gRPC service implementation for collecting telemetry data from data-plane nodes.
 *
 * <p>Receives streaming stats reports from nodes and responds with ack messages
 * that can adjust the reporting interval. Currently logs stats at DEBUG level;
 * future implementations should forward to a metrics store (Prometheus, InfluxDB, etc.).</p>
 *
 * <p>Thread safety: each streaming call operates on its own pair of {@link StreamObserver}
 * instances. The injected {@link NodeRegistry} is itself thread-safe.</p>
 */
@Log4j2
public final class StatsCollectionServiceImpl
        extends StatsCollectionServiceGrpc.StatsCollectionServiceImplBase {

    /**
     * Default interval (in milliseconds) between stats reports.
     * Sent to nodes in each {@link StatsAck} response.
     */
    private static final long DEFAULT_REPORT_INTERVAL_MS = 30_000;

    private final NodeRegistry registry;

    /**
     * @param registry the node registry for session validation; must not be null
     */
    public StatsCollectionServiceImpl(NodeRegistry registry) {
        this.registry = Objects.requireNonNull(registry, "registry");
    }

    /**
     * Bidirectional stats reporting stream. Nodes send periodic stats reports;
     * the control plane responds with {@link StatsAck} messages that include
     * the desired next report interval.
     */
    @Override
    public StreamObserver<NodeStatsReport> reportStats(StreamObserver<StatsAck> responseObserver) {
        return new StreamObserver<>() {

            private volatile String nodeId;

            @Override
            public void onNext(NodeStatsReport report) {
                String reportNodeId = report.getNodeId();
                String sessionToken = report.getSessionToken();

                if (!registry.validateSession(reportNodeId, sessionToken)) {
                    log.warn("Stats report rejected: invalid session for node {}", reportNodeId);
                    responseObserver.onError(Status.UNAUTHENTICATED
                            .withDescription("Invalid session token for node: " + reportNodeId)
                            .asRuntimeException());
                    return;
                }

                this.nodeId = reportNodeId;

                // Log at DEBUG level for now. In production, forward to metrics pipeline.
                if (log.isDebugEnabled()) {
                    log.debug("Stats from node {}: clusters={}, listeners={}, system=[cpu={}%, mem={}/{}]",
                            reportNodeId,
                            report.getClusterStatsCount(),
                            report.getListenerStatsCount(),
                            report.hasSystemStats()
                                    ? String.format("%.1f", report.getSystemStats().getCpuUsagePercent())
                                    : "N/A",
                            report.hasSystemStats()
                                    ? report.getSystemStats().getMemoryUsedBytes()
                                    : "N/A",
                            report.hasSystemStats()
                                    ? report.getSystemStats().getMemoryTotalBytes()
                                    : "N/A");
                }

                StatsAck ack = StatsAck.newBuilder()
                        .setTimestamp(Timestamp.newBuilder()
                                .setEpochMillis(System.currentTimeMillis())
                                .build())
                        .setNextReportIntervalMs(DEFAULT_REPORT_INTERVAL_MS)
                        .setReduceDetailLevel(false)
                        .build();

                responseObserver.onNext(ack);
            }

            @Override
            public void onError(Throwable t) {
                String id = this.nodeId;
                if (id != null) {
                    log.warn("Stats stream error for node {}: {}", id, t.getMessage());
                } else {
                    log.warn("Stats stream error from unknown node: {}", t.getMessage());
                }
            }

            @Override
            public void onCompleted() {
                String id = this.nodeId;
                if (id != null) {
                    log.info("Stats stream completed for node {}", id);
                }
                responseObserver.onCompleted();
            }
        };
    }
}
