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
package com.shieldblaze.expressgateway.controlplane.resilience;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.time.Instant;
import java.util.Collections;
import java.util.HexFormat;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * Detects config corruption via checksums, manages automatic rollback to the last
 * known good configuration, and maintains an audit trail of corruption events.
 *
 * <h3>Corruption detection</h3>
 * <p>Every config snapshot stored in the KV store or local cache includes a SHA-256
 * checksum. When config is read, the checksum is recomputed and compared. A mismatch
 * indicates corruption (storage failure, partial write, bit rot).</p>
 *
 * <h3>Rollback</h3>
 * <p>On corruption detection, the handler invokes the registered {@link RollbackCallback}
 * to restore the last known good configuration. The callback is responsible for the
 * actual restoration logic (reading from local cache or requesting from a peer).</p>
 *
 * <h3>Audit trail</h3>
 * <p>All corruption events are recorded in an in-memory audit log with timestamps,
 * affected resource paths, and whether rollback succeeded.</p>
 *
 * <p>Thread safety: all public methods are safe for concurrent use.</p>
 */
public final class ConfigCorruptionHandler {

    private static final Logger logger = LogManager.getLogger(ConfigCorruptionHandler.class);

    /**
     * A recorded corruption event.
     *
     * @param timestamp       when the corruption was detected
     * @param resourcePath    the path of the corrupted resource (or "snapshot" for full snapshots)
     * @param expectedChecksum the expected checksum
     * @param actualChecksum   the actual checksum computed from data
     * @param rollbackAttempted whether a rollback was attempted
     * @param rollbackSuccess  whether the rollback succeeded
     */
    public record CorruptionEvent(
            Instant timestamp,
            String resourcePath,
            String expectedChecksum,
            String actualChecksum,
            boolean rollbackAttempted,
            boolean rollbackSuccess
    ) {
        public CorruptionEvent {
            Objects.requireNonNull(timestamp, "timestamp");
            Objects.requireNonNull(resourcePath, "resourcePath");
            Objects.requireNonNull(expectedChecksum, "expectedChecksum");
            Objects.requireNonNull(actualChecksum, "actualChecksum");
        }
    }

    /**
     * Callback for performing rollback to the last known good configuration.
     */
    @FunctionalInterface
    public interface RollbackCallback {
        /**
         * Attempts to roll back to the last known good configuration.
         *
         * @param corruptedPath the path of the corrupted resource
         * @return true if rollback succeeded
         */
        boolean rollback(String corruptedPath);
    }

    /**
     * Listener for corruption alerts.
     */
    @FunctionalInterface
    public interface CorruptionAlertListener {
        void onCorruption(CorruptionEvent event);
    }

    private final RollbackCallback rollbackCallback;
    private final CopyOnWriteArrayList<CorruptionAlertListener> alertListeners = new CopyOnWriteArrayList<>();
    private final CopyOnWriteArrayList<CorruptionEvent> auditLog = new CopyOnWriteArrayList<>();
    private final int maxAuditLogSize;

    /**
     * Creates a new corruption handler.
     *
     * @param rollbackCallback the callback for performing rollback; must not be null
     * @param maxAuditLogSize  maximum number of events to keep in the audit trail
     */
    public ConfigCorruptionHandler(RollbackCallback rollbackCallback, int maxAuditLogSize) {
        this.rollbackCallback = Objects.requireNonNull(rollbackCallback, "rollbackCallback");
        if (maxAuditLogSize < 1) {
            throw new IllegalArgumentException("maxAuditLogSize must be >= 1");
        }
        this.maxAuditLogSize = maxAuditLogSize;
    }

    /**
     * Creates a new corruption handler with a default audit log size of 1000.
     */
    public ConfigCorruptionHandler(RollbackCallback rollbackCallback) {
        this(rollbackCallback, 1000);
    }

    /**
     * Adds an alert listener.
     */
    public void addAlertListener(CorruptionAlertListener listener) {
        Objects.requireNonNull(listener, "listener");
        alertListeners.add(listener);
    }

    /**
     * Verifies the integrity of config data against its expected checksum.
     *
     * @param resourcePath     the resource path for logging/audit
     * @param data             the config data bytes
     * @param expectedChecksum the expected SHA-256 hex checksum
     * @return true if the data is intact, false if corruption is detected
     */
    public boolean verify(String resourcePath, byte[] data, String expectedChecksum) {
        Objects.requireNonNull(resourcePath, "resourcePath");
        Objects.requireNonNull(data, "data");
        Objects.requireNonNull(expectedChecksum, "expectedChecksum");

        String actualChecksum = sha256(data);
        if (actualChecksum.equals(expectedChecksum)) {
            return true;
        }

        // Corruption detected
        logger.error("Config corruption detected for {}: expected={}, actual={}",
                resourcePath, expectedChecksum, actualChecksum);

        boolean rollbackSuccess = false;
        try {
            rollbackSuccess = rollbackCallback.rollback(resourcePath);
            if (rollbackSuccess) {
                logger.info("Rollback succeeded for corrupted resource: {}", resourcePath);
            } else {
                logger.error("Rollback FAILED for corrupted resource: {}", resourcePath);
            }
        } catch (Exception e) {
            logger.error("Rollback threw exception for resource: {}", resourcePath, e);
        }

        CorruptionEvent event = new CorruptionEvent(
                Instant.now(), resourcePath, expectedChecksum, actualChecksum, true, rollbackSuccess);
        recordEvent(event);
        fireAlerts(event);

        return false;
    }

    /**
     * Computes the SHA-256 checksum of the given data.
     *
     * @param data the data to checksum
     * @return the hex-encoded SHA-256 digest
     */
    public static String computeChecksum(byte[] data) {
        return sha256(data);
    }

    /**
     * Returns the audit log of corruption events.
     */
    public List<CorruptionEvent> auditLog() {
        return Collections.unmodifiableList(auditLog);
    }

    /**
     * Returns the number of corruption events recorded.
     */
    public int corruptionCount() {
        return auditLog.size();
    }

    private void recordEvent(CorruptionEvent event) {
        auditLog.add(event);
        // Trim if over max size (remove oldest entries)
        while (auditLog.size() > maxAuditLogSize) {
            auditLog.remove(0);
        }
    }

    private void fireAlerts(CorruptionEvent event) {
        for (CorruptionAlertListener listener : alertListeners) {
            try {
                listener.onCorruption(event);
            } catch (Exception e) {
                logger.warn("Corruption alert listener threw exception", e);
            }
        }
    }

    private static String sha256(byte[] data) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            byte[] hash = digest.digest(data);
            return HexFormat.of().formatHex(hash);
        } catch (NoSuchAlgorithmException e) {
            throw new AssertionError("SHA-256 not available", e);
        }
    }
}
