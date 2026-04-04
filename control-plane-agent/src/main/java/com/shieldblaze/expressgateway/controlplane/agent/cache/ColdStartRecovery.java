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
package com.shieldblaze.expressgateway.controlplane.agent.cache;

import lombok.extern.slf4j.Slf4j;

import java.util.List;
import java.util.Objects;
import java.util.Optional;

/**
 * Handles cold start recovery by loading the best available config from the
 * {@link LocalConfigCache} when the control plane is unreachable.
 *
 * <h3>Fallback chain</h3>
 * <ol>
 *   <li><b>Latest version</b> -- the most recent config version in the cache</li>
 *   <li><b>Previous version</b> -- the second-most-recent version (latest may be corrupt)</li>
 *   <li><b>Tagged version</b> -- a version tagged with "known-good" as a manual rollback point</li>
 *   <li><b>Default config</b> -- a caller-supplied default (may be null for no-config startup)</li>
 * </ol>
 *
 * <p>Before using any cached version, its integrity is verified via SHA-256 checksum.
 * Corrupted versions are skipped and the fallback chain continues.</p>
 *
 * <p>After recovery completes, the recovery status is reported for monitoring/alerting.</p>
 */
@Slf4j
public final class ColdStartRecovery {

    /**
     * Recovery outcome.
     */
    public enum RecoveryStatus {
        /** Recovered from the latest cached version. */
        RECOVERED_LATEST,
        /** Recovered from a previous (non-latest) cached version. */
        RECOVERED_PREVIOUS,
        /** Recovered from a tagged "known-good" version. */
        RECOVERED_TAGGED,
        /** Used the default config (no valid cache). */
        USED_DEFAULT,
        /** No recovery possible: no cache and no default. */
        FAILED
    }

    /**
     * Result of a cold start recovery attempt.
     *
     * @param status     the recovery outcome
     * @param data       the config data bytes (null if FAILED)
     * @param version    the config version used (null if USED_DEFAULT or FAILED)
     * @param message    a human-readable description of what happened
     */
    public record RecoveryResult(
            RecoveryStatus status,
            byte[] data,
            ConfigVersion version,
            String message
    ) {
        public RecoveryResult {
            Objects.requireNonNull(status, "status");
            Objects.requireNonNull(message, "message");
        }

        /**
         * Returns true if the recovery loaded usable configuration data.
         */
        public boolean hasData() {
            return data != null && data.length > 0;
        }
    }

    private static final String KNOWN_GOOD_TAG = "known-good";

    private final LocalConfigCache cache;

    /**
     * Creates a new cold start recovery handler.
     *
     * @param cache the local config cache to recover from
     */
    public ColdStartRecovery(LocalConfigCache cache) {
        this.cache = Objects.requireNonNull(cache, "cache");
    }

    /**
     * Attempts to recover configuration from the local cache.
     *
     * @param defaultConfig the default config to use as a last resort (may be null)
     * @return the recovery result
     */
    public RecoveryResult recover(byte[] defaultConfig) {
        log.info("Starting cold start recovery");

        // Step 0: Verify integrity and purge corrupted versions
        List<Long> corrupted = cache.verifyIntegrity();
        if (!corrupted.isEmpty()) {
            log.warn("Found {} corrupted versions during cold start, purging", corrupted.size());
            try {
                cache.purgeCorrupted(corrupted);
            } catch (Exception e) {
                log.error("Failed to purge corrupted versions during cold start", e);
            }
        }

        // Step 1: Try latest version
        Optional<ConfigVersion> latest = cache.latestVersion();
        if (latest.isPresent()) {
            Optional<byte[]> data = cache.load(latest.get().revision());
            if (data.isPresent()) {
                log.info("Cold start recovery: loaded latest version (revision={})",
                        latest.get().revision());
                return new RecoveryResult(RecoveryStatus.RECOVERED_LATEST, data.get(),
                        latest.get(), "Recovered from latest cached version at revision " + latest.get().revision());
            }
            log.warn("Latest version (revision={}) failed integrity check", latest.get().revision());
        }

        // Step 2: Try previous versions in descending order
        List<ConfigVersion> allVersions = cache.allVersions();
        for (int i = allVersions.size() - 2; i >= 0; i--) {
            ConfigVersion prev = allVersions.get(i);
            Optional<byte[]> data = cache.load(prev.revision());
            if (data.isPresent()) {
                log.info("Cold start recovery: loaded previous version (revision={})", prev.revision());
                return new RecoveryResult(RecoveryStatus.RECOVERED_PREVIOUS, data.get(),
                        prev, "Recovered from previous cached version at revision " + prev.revision());
            }
        }

        // Step 3: Try known-good tagged version
        Optional<ConfigVersion> knownGood = cache.findByTag(KNOWN_GOOD_TAG);
        if (knownGood.isPresent()) {
            Optional<byte[]> data = cache.load(knownGood.get().revision());
            if (data.isPresent()) {
                log.info("Cold start recovery: loaded known-good tagged version (revision={})",
                        knownGood.get().revision());
                return new RecoveryResult(RecoveryStatus.RECOVERED_TAGGED, data.get(),
                        knownGood.get(), "Recovered from known-good tagged version at revision " +
                        knownGood.get().revision());
            }
        }

        // Step 4: Use default config
        if (defaultConfig != null && defaultConfig.length > 0) {
            log.warn("Cold start recovery: no valid cached config found, using default");
            return new RecoveryResult(RecoveryStatus.USED_DEFAULT, defaultConfig,
                    null, "No valid cached config available, using default configuration");
        }

        // Step 5: Complete failure
        log.error("Cold start recovery FAILED: no cached config and no default available");
        return new RecoveryResult(RecoveryStatus.FAILED, null, null,
                "No cached config and no default config available. Node will start without configuration.");
    }
}
