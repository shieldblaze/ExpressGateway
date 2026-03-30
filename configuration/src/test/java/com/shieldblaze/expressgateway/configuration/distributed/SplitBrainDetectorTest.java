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

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import org.apache.curator.framework.state.ConnectionState;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Path;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * RB-TEST-03: Unit tests for {@link SplitBrainDetector}.
 *
 * <p>Validates the ZooKeeper connection state machine transitions without
 * requiring a real ZooKeeper instance. The detector's state transitions are
 * tested by directly invoking {@link SplitBrainDetector#stateChanged} with
 * different {@link ConnectionState} values.</p>
 *
 * <p>State transitions tested:
 * <ul>
 *   <li>{@code CONNECTED}: healthy, not read-only</li>
 *   <li>{@code RECONNECTED}: healthy, not read-only</li>
 *   <li>{@code SUSPENDED}: not healthy, read-only (reject config changes)</li>
 *   <li>{@code READ_ONLY}: not healthy, read-only</li>
 *   <li>{@code LOST}: not healthy, read-only, triggers fallback callback</li>
 * </ul>
 */
@Timeout(value = 10)
class SplitBrainDetectorTest {

    @TempDir
    Path tempDir;

    @Test
    void initialState_isHealthy() {
        SplitBrainDetector detector = createDetector(new AtomicReference<>());

        assertTrue(detector.isHealthy(), "Initial state must be healthy (CONNECTED)");
        assertFalse(detector.isReadOnly(), "Initial state must not be read-only");
    }

    @Test
    void connected_isHealthy() {
        SplitBrainDetector detector = createDetector(new AtomicReference<>());

        detector.stateChanged(null, ConnectionState.CONNECTED);
        assertTrue(detector.isHealthy());
        assertFalse(detector.isReadOnly());
    }

    @Test
    void reconnected_isHealthy() {
        SplitBrainDetector detector = createDetector(new AtomicReference<>());

        // Simulate disconnect then reconnect
        detector.stateChanged(null, ConnectionState.SUSPENDED);
        assertFalse(detector.isHealthy());

        detector.stateChanged(null, ConnectionState.RECONNECTED);
        assertTrue(detector.isHealthy(), "RECONNECTED must restore healthy state");
        assertFalse(detector.isReadOnly());
    }

    @Test
    void suspended_isReadOnly() {
        SplitBrainDetector detector = createDetector(new AtomicReference<>());

        detector.stateChanged(null, ConnectionState.SUSPENDED);
        assertFalse(detector.isHealthy(), "SUSPENDED must not be healthy");
        assertTrue(detector.isReadOnly(), "SUSPENDED must be read-only");
    }

    @Test
    void readOnly_isReadOnly() {
        SplitBrainDetector detector = createDetector(new AtomicReference<>());

        detector.stateChanged(null, ConnectionState.READ_ONLY);
        assertFalse(detector.isHealthy(), "READ_ONLY must not be healthy");
        assertTrue(detector.isReadOnly(), "READ_ONLY must be read-only");
    }

    @Test
    void lost_isReadOnly_andTriggersFallback() throws IOException {
        AtomicReference<ConfigurationContext> fallbackReceived = new AtomicReference<>();
        ConfigFallbackStore fallbackStore = new ConfigFallbackStore(tempDir);
        // Save a config so it can be loaded as LKG
        fallbackStore.saveLastKnownGood(ConfigurationContext.DEFAULT);

        SplitBrainDetector detector = new SplitBrainDetector(fallbackStore, fallbackReceived::set);

        detector.stateChanged(null, ConnectionState.LOST);
        assertFalse(detector.isHealthy(), "LOST must not be healthy");
        assertTrue(detector.isReadOnly(), "LOST must be read-only");
        assertNotNull(fallbackReceived.get(),
                "LOST must trigger fallback callback with last-known-good config");
    }

    @Test
    void lost_withNoLkg_doesNotCallFallback() {
        AtomicReference<ConfigurationContext> fallbackReceived = new AtomicReference<>();
        ConfigFallbackStore emptyStore = new ConfigFallbackStore(tempDir);
        // No LKG saved

        SplitBrainDetector detector = new SplitBrainDetector(emptyStore, fallbackReceived::set);

        detector.stateChanged(null, ConnectionState.LOST);
        assertNull(fallbackReceived.get(),
                "LOST with no LKG must not invoke fallback callback");
    }

    @Test
    void fullLifecycle_connectedSuspendedReconnectedLost() throws IOException {
        AtomicReference<ConfigurationContext> fallbackReceived = new AtomicReference<>();
        ConfigFallbackStore fallbackStore = new ConfigFallbackStore(tempDir);
        fallbackStore.saveLastKnownGood(ConfigurationContext.DEFAULT);

        SplitBrainDetector detector = new SplitBrainDetector(fallbackStore, fallbackReceived::set);

        // Start healthy
        assertTrue(detector.isHealthy());
        assertFalse(detector.isReadOnly());

        // SUSPENDED -- ZK session timeout approaching
        detector.stateChanged(null, ConnectionState.SUSPENDED);
        assertFalse(detector.isHealthy());
        assertTrue(detector.isReadOnly());
        assertNull(fallbackReceived.get(), "SUSPENDED must not trigger fallback");

        // RECONNECTED -- session recovered before timeout
        detector.stateChanged(null, ConnectionState.RECONNECTED);
        assertTrue(detector.isHealthy());
        assertFalse(detector.isReadOnly());

        // LOST -- session expired
        detector.stateChanged(null, ConnectionState.LOST);
        assertFalse(detector.isHealthy());
        assertTrue(detector.isReadOnly());
        assertNotNull(fallbackReceived.get(), "LOST must trigger fallback");
    }

    @Test
    void close_doesNotThrow() {
        SplitBrainDetector detector = createDetector(new AtomicReference<>());
        detector.close(); // Must not throw
    }

    private SplitBrainDetector createDetector(AtomicReference<ConfigurationContext> fallbackRef) {
        ConfigFallbackStore fallbackStore = new ConfigFallbackStore(tempDir);
        return new SplitBrainDetector(fallbackStore, fallbackRef::set);
    }
}
