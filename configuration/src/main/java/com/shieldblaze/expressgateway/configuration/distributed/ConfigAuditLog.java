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
package com.shieldblaze.expressgateway.configuration.distributed;

import com.fasterxml.jackson.databind.node.ObjectNode;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.common.zookeeper.ZNodePath;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.time.ZoneOffset;
import java.time.format.DateTimeFormatter;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;

/**
 * Immutable audit log for all configuration changes in the distributed system,
 * backed by a {@link ConfigStorageBackend}.
 *
 * <p>Stores audit entries as persistent sequential keys at:</p>
 * <pre>
 * /ExpressGateway/{env}/{clusterId}/config/audit/entry-0000000001
 * /ExpressGateway/{env}/{clusterId}/config/audit/entry-0000000002
 * </pre>
 *
 * <p>Each entry is a JSON object containing:</p>
 * <pre>
 * {
 *   "timestamp": "2026-03-24T10:30:00Z",
 *   "version": "v005",
 *   "action": "PROPOSE",
 *   "nodeId": "node-1",
 *   "reason": "operator-initiated",
 *   "previousVersion": "v004"
 * }
 * </pre>
 *
 * <p>Writes are fire-and-forget with retry, executed on a dedicated single-thread
 * executor to avoid blocking the caller. Old entries beyond the configured retention
 * count are trimmed asynchronously after each write.</p>
 */
public final class ConfigAuditLog implements Closeable {

    private static final Logger logger = LogManager.getLogger(ConfigAuditLog.class);

    private static final String ROOT_PATH = "ExpressGateway";
    private static final String CONFIG_COMPONENT = "config";
    private static final String AUDIT_COMPONENT = "audit";
    private static final String ENTRY_PREFIX = "entry-";

    private static final int DEFAULT_RETENTION_COUNT = 1000;
    private static final int MAX_WRITE_RETRIES = 3;

    private final String clusterId;
    private final Environment environment;
    private final String nodeId;
    private final int retentionCount;
    private final ConfigStorageBackend storageBackend;

    private final ExecutorService writeExecutor;
    private final AtomicBoolean closed;

    /**
     * Creates a new {@link ConfigAuditLog} with default retention of 1000 entries.
     *
     * @param clusterId      The cluster identifier
     * @param environment    The deployment environment
     * @param nodeId         The unique identifier for this node
     * @param storageBackend The storage backend
     */
    public ConfigAuditLog(String clusterId, Environment environment, String nodeId,
                          ConfigStorageBackend storageBackend) {
        this(clusterId, environment, nodeId, DEFAULT_RETENTION_COUNT, storageBackend);
    }

    /**
     * Creates a new {@link ConfigAuditLog}.
     *
     * @param clusterId      The cluster identifier
     * @param environment    The deployment environment
     * @param nodeId         The unique identifier for this node
     * @param retentionCount The maximum number of audit entries to retain
     * @param storageBackend The storage backend
     */
    public ConfigAuditLog(String clusterId, Environment environment, String nodeId, int retentionCount,
                          ConfigStorageBackend storageBackend) {
        this.clusterId = Objects.requireNonNull(clusterId, "clusterId");
        this.environment = Objects.requireNonNull(environment, "environment");
        this.nodeId = Objects.requireNonNull(nodeId, "nodeId");
        if (retentionCount < 1) {
            throw new IllegalArgumentException("retentionCount must be >= 1, got: " + retentionCount);
        }
        this.retentionCount = retentionCount;
        this.storageBackend = Objects.requireNonNull(storageBackend, "storageBackend");
        this.writeExecutor = Executors.newSingleThreadExecutor(r -> {
            Thread t = new Thread(r, "config-audit-writer");
            t.setDaemon(true);
            return t;
        });
        this.closed = new AtomicBoolean(false);
    }

    /**
     * Log a configuration action asynchronously (fire-and-forget with retry).
     *
     * @param version         The configuration version (e.g. "v005")
     * @param action          The action performed
     * @param reason          Human-readable reason for the action
     * @param previousVersion The previous configuration version (e.g. "v004"), or {@code null} if none
     */
    public void log(String version, AuditAction action, String reason, String previousVersion) {
        Objects.requireNonNull(version, "version");
        Objects.requireNonNull(action, "action");

        if (closed.get()) {
            logger.warn("Audit log is closed, dropping entry for version {} action {}", version, action);
            return;
        }

        // Build the JSON entry eagerly on the caller's thread to capture timestamp accurately
        String json = buildEntryJson(version, action, reason, previousVersion);

        writeExecutor.execute(() -> writeWithRetry(json, version, action));
    }

    /**
     * Read all audit entries, ordered by sequence number (oldest first).
     *
     * @return An unmodifiable list of audit entry JSON strings
     * @throws Exception If an error occurs reading from the backend
     */
    public List<String> readAll() throws Exception {
        String auditPath = auditBasePath();

        if (!storageBackend.exists(auditPath)) {
            return Collections.emptyList();
        }

        List<String> children = storageBackend.listChildren(auditPath);
        Collections.sort(children); // Sequential keys are lexicographically ordered

        List<String> entries = new ArrayList<>(children.size());
        for (String child : children) {
            try {
                Optional<byte[]> data = storageBackend.get(auditPath + "/" + child);
                data.ifPresent(bytes -> entries.add(new String(bytes, StandardCharsets.UTF_8)));
            } catch (Exception e) {
                logger.warn("Failed to read audit entry {}, skipping", child, e);
            }
        }

        return Collections.unmodifiableList(entries);
    }

    /**
     * Read the most recent N audit entries (newest first).
     *
     * @param count The maximum number of entries to return
     * @return An unmodifiable list of audit entry JSON strings, newest first
     * @throws Exception If an error occurs reading from the backend
     */
    public List<String> readRecent(int count) throws Exception {
        if (count < 1) {
            throw new IllegalArgumentException("count must be >= 1");
        }

        String auditPath = auditBasePath();

        if (!storageBackend.exists(auditPath)) {
            return Collections.emptyList();
        }

        List<String> children = storageBackend.listChildren(auditPath);
        Collections.sort(children);

        // Take only the last N entries and reverse
        int fromIndex = Math.max(0, children.size() - count);
        List<String> recent = new ArrayList<>(children.subList(fromIndex, children.size()));
        Collections.reverse(recent);

        List<String> entries = new ArrayList<>(recent.size());
        for (String child : recent) {
            try {
                Optional<byte[]> data = storageBackend.get(auditPath + "/" + child);
                data.ifPresent(bytes -> entries.add(new String(bytes, StandardCharsets.UTF_8)));
            } catch (Exception e) {
                logger.warn("Failed to read audit entry {}, skipping", child, e);
            }
        }

        return Collections.unmodifiableList(entries);
    }

    private String buildEntryJson(String version, AuditAction action, String reason, String previousVersion) {
        ObjectNode node = OBJECT_MAPPER.createObjectNode();
        node.put("timestamp", DateTimeFormatter.ISO_INSTANT.format(Instant.now().atOffset(ZoneOffset.UTC)));
        node.put("version", version);
        node.put("action", action.name());
        node.put("nodeId", nodeId);
        node.put("reason", reason != null ? reason : "");
        node.put("previousVersion", previousVersion != null ? previousVersion : "");
        return node.toString();
    }

    private void writeWithRetry(String json, String version, AuditAction action) {
        for (int attempt = 1; attempt <= MAX_WRITE_RETRIES; attempt++) {
            try {
                String auditPath = auditBasePath();

                // Ensure parent path exists
                storageBackend.put(auditPath, new byte[0]);

                // Create sequential persistent key for the audit entry
                storageBackend.putSequential(auditPath + "/" + ENTRY_PREFIX, json.getBytes(StandardCharsets.UTF_8));

                logger.debug("Audit log entry written: version={}, action={}", version, action);

                // Trim old entries beyond retention count
                trimOldEntries(auditPath);
                return;

            } catch (Exception e) {
                if (attempt < MAX_WRITE_RETRIES) {
                    logger.warn("Failed to write audit log entry (attempt {}/{}): version={}, action={}",
                            attempt, MAX_WRITE_RETRIES, version, action, e);
                } else {
                    logger.error("Failed to write audit log entry after {} attempts: version={}, action={}",
                            MAX_WRITE_RETRIES, version, action, e);
                }
            }
        }
    }

    private void trimOldEntries(String auditPath) {
        try {
            List<String> children = storageBackend.listChildren(auditPath);
            if (children.size() <= retentionCount) {
                return;
            }

            List<String> sorted = new ArrayList<>(children);
            Collections.sort(sorted);
            int toDelete = sorted.size() - retentionCount;

            for (int i = 0; i < toDelete; i++) {
                try {
                    storageBackend.delete(auditPath + "/" + sorted.get(i));
                } catch (Exception e) {
                    logger.warn("Failed to trim audit entry {}", sorted.get(i), e);
                }
            }

            logger.debug("Trimmed {} old audit entries, {} remaining", toDelete, retentionCount);
        } catch (Exception e) {
            logger.warn("Failed to trim audit log entries", e);
        }
    }

    private String auditBasePath() {
        return ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT, AUDIT_COMPONENT).path();
    }

    @Override
    public void close() throws IOException {
        if (closed.compareAndSet(false, true)) {
            writeExecutor.shutdown();
            try {
                if (!writeExecutor.awaitTermination(5, TimeUnit.SECONDS)) {
                    writeExecutor.shutdownNow();
                }
            } catch (InterruptedException e) {
                writeExecutor.shutdownNow();
                Thread.currentThread().interrupt();
            }
            logger.info("Closed ConfigAuditLog for node {}", nodeId);
        }
    }

    /**
     * Actions that can be audited.
     */
    public enum AuditAction {
        /** A new configuration version has been proposed. */
        PROPOSE,
        /** A configuration version has been committed (quorum reached). */
        COMMIT,
        /** A configuration version has been rolled back. */
        ROLLBACK,
        /** A configuration version has been rejected (validation failure, quorum timeout). */
        REJECT,
        /** Crash recovery detected and resolved an orphaned proposal. */
        RECOVERY
    }
}
