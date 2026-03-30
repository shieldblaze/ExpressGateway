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

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;
import java.util.Objects;

/**
 * Handles recovery from leader crashes during configuration proposals.
 *
 * <p>When the leader crashes between writing a new config version and updating the
 * {@code /current} pointer (or after updating the pointer but before quorum is confirmed),
 * orphaned proposals are left in ZooKeeper. This manager detects and resolves them.</p>
 *
 * <p>Recovery logic on leader startup:</p>
 * <ol>
 *   <li>List all versions and find the current pointer version.</li>
 *   <li>If the highest version is greater than the current pointer version,
 *       an orphaned proposal exists.</li>
 *   <li>If the orphaned version has quorum ACKs (majority), complete the commit
 *       by updating the current pointer.</li>
 *   <li>If the orphaned version does NOT have quorum ACKs, roll back by leaving
 *       the current pointer unchanged (the orphaned version is effectively abandoned).</li>
 *   <li>All recovery actions are logged to the audit trail.</li>
 * </ol>
 */
public final class CrashRecoveryManager {

    private static final Logger logger = LogManager.getLogger(CrashRecoveryManager.class);

    private final ConfigVersionStore versionStore;
    private final ConfigQuorumManager quorumManager;
    private final ConfigAuditLog auditLog;
    private final ConfigFallbackStore fallbackStore;
    private final int totalNodes;

    /**
     * Creates a new {@link CrashRecoveryManager}.
     *
     * @param versionStore  The versioned config store
     * @param quorumManager The quorum manager for checking ACKs
     * @param auditLog      The audit log for recording recovery actions
     * @param fallbackStore The fallback store for saving LKG config
     * @param totalNodes    The total number of nodes in the cluster (for quorum calculation)
     */
    public CrashRecoveryManager(ConfigVersionStore versionStore,
                                ConfigQuorumManager quorumManager,
                                ConfigAuditLog auditLog,
                                ConfigFallbackStore fallbackStore,
                                int totalNodes) {
        this.versionStore = Objects.requireNonNull(versionStore, "versionStore");
        this.quorumManager = Objects.requireNonNull(quorumManager, "quorumManager");
        this.auditLog = Objects.requireNonNull(auditLog, "auditLog");
        this.fallbackStore = Objects.requireNonNull(fallbackStore, "fallbackStore");
        if (totalNodes < 1) {
            throw new IllegalArgumentException("totalNodes must be >= 1, got: " + totalNodes);
        }
        this.totalNodes = totalNodes;
    }

    /**
     * Called on leader startup to recover from incomplete proposals.
     *
     * <p>Scans for versions that exist beyond the current pointer. If found, either
     * completes the commit (if quorum ACKs exist) or abandons the orphaned version
     * (if quorum was not reached).</p>
     *
     * <p>This method is idempotent: calling it when no orphaned proposals exist is a no-op.</p>
     */
    public void recoverIfNeeded() {
        try {
            logger.info("Checking for orphaned configuration proposals...");

            // CRITICAL-1 fix: Wait for a stabilization period before running recovery.
            // After a leader crash, the old leader's ephemeral ACK znodes are still
            // visible until the ZK session timeout expires (typically 30-60s). If we
            // read ACK counts immediately, we could see stale ACKs from the crashed
            // leader and incorrectly commit an orphaned version. The default wait
            // (35s) exceeds the typical ZK session timeout (30s) to ensure that
            // stale ephemeral znodes have been cleaned up before we count ACKs.
            // Override via -Dexpressgateway.recovery.stabilization.ms for custom ZK configs.
            long stabilizationMs = Long.getLong("expressgateway.recovery.stabilization.ms", 35000);
            if (stabilizationMs > 0) {
                logger.info("Waiting {}ms for leader stabilization before crash recovery...", stabilizationMs);
                Thread.sleep(stabilizationMs);
            }

            int currentVersion = versionStore.currentVersion();
            List<Integer> allVersions = versionStore.listVersions();

            if (allVersions.isEmpty()) {
                logger.info("No configuration versions found, nothing to recover");
                return;
            }

            int highestVersion = allVersions.get(allVersions.size() - 1);

            // No current pointer set at all -- the very first proposal may have crashed
            // before setCurrent was called
            if (currentVersion < 0) {
                logger.info("No current version pointer found. Checking if an initial proposal needs recovery.");
                recoverOrphanedVersion(highestVersion, -1);
                return;
            }

            // Normal case: check if there are versions beyond current
            if (highestVersion > currentVersion) {
                logger.warn("Detected orphaned proposal(s): current={}, highest={}",
                        currentVersion, highestVersion);

                // Recover the latest orphaned version only. Intermediate orphans (if multiple
                // crashed proposals occurred) are implicitly abandoned since only the most
                // recent one could potentially have quorum.
                recoverOrphanedVersion(highestVersion, currentVersion);
            } else {
                logger.info("No orphaned proposals detected (current={}, highest={})",
                        currentVersion, highestVersion);
            }

        } catch (Exception e) {
            logger.error("Error during crash recovery check", e);
            auditLog.log("unknown", ConfigAuditLog.AuditAction.RECOVERY,
                    "crash-recovery-failed: " + e.getMessage(), "unknown");
        }
    }

    private void recoverOrphanedVersion(int orphanedVersion, int currentVersion) {
        String orphanedVersionStr = ConfigQuorumManager.formatVersion(orphanedVersion);
        String currentVersionStr = currentVersion > 0 ? ConfigQuorumManager.formatVersion(currentVersion) : "none";

        try {
            int requiredAcks = ConfigQuorumManager.quorumSize(totalNodes);
            int actualAcks = quorumManager.getAckCount(orphanedVersionStr);

            logger.info("Orphaned version {} has {}/{} ACKs (quorum requires {})",
                    orphanedVersionStr, actualAcks, totalNodes, requiredAcks);

            if (actualAcks >= requiredAcks) {
                // Quorum was reached -- complete the commit
                logger.info("Completing interrupted commit for version {} (quorum met)", orphanedVersionStr);

                versionStore.setCurrent(orphanedVersion);

                // Update LKG
                try {
                    var config = versionStore.readVersion(orphanedVersion);
                    fallbackStore.saveLastKnownGood(config);
                } catch (Exception e) {
                    logger.warn("Failed to update LKG after recovery commit of version {}", orphanedVersionStr, e);
                }

                auditLog.log(orphanedVersionStr, ConfigAuditLog.AuditAction.RECOVERY,
                        "crash-recovery-commit: quorum met (" + actualAcks + "/" + requiredAcks + ")",
                        currentVersionStr);

                logger.info("Recovery: committed orphaned version {}", orphanedVersionStr);

            } else {
                // Quorum NOT reached -- abandon the orphaned version, keep current pointer
                logger.info("Abandoning orphaned version {} (quorum not met: {}/{})",
                        orphanedVersionStr, actualAcks, requiredAcks);

                // Clean up ACKs for the abandoned version
                quorumManager.cleanupAcks(orphanedVersionStr);

                auditLog.log(orphanedVersionStr, ConfigAuditLog.AuditAction.RECOVERY,
                        "crash-recovery-abandon: quorum not met (" + actualAcks + "/" + requiredAcks
                                + "), keeping current=" + currentVersionStr,
                        currentVersionStr);

                logger.info("Recovery: abandoned orphaned version {}, current remains {}",
                        orphanedVersionStr, currentVersionStr);
            }

        } catch (Exception e) {
            logger.error("Failed to recover orphaned version {}", orphanedVersionStr, e);
            auditLog.log(orphanedVersionStr, ConfigAuditLog.AuditAction.RECOVERY,
                    "crash-recovery-error: " + e.getMessage(), currentVersionStr);
        }
    }
}
