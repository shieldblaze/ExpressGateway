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
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Path;
import java.util.Optional;

import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * RB-TEST-03 (supplementary): Unit tests for {@link ConfigFallbackStore}.
 *
 * <p>Validates the last-known-good configuration persistence and recovery
 * mechanism used during ZooKeeper connection loss.</p>
 */
@Timeout(value = 10)
class ConfigFallbackStoreTest {

    @TempDir
    Path tempDir;

    @Test
    void saveAndLoad_roundTrip() throws Exception {
        ConfigFallbackStore store = new ConfigFallbackStore(tempDir);

        store.saveLastKnownGood(ConfigurationContext.DEFAULT);

        Optional<ConfigurationContext> loaded = store.loadLastKnownGood();
        assertTrue(loaded.isPresent(), "Must load saved LKG configuration");
        assertNotNull(loaded.get());
    }

    @Test
    void load_withNoSavedConfig_returnsEmpty() {
        ConfigFallbackStore store = new ConfigFallbackStore(tempDir);

        Optional<ConfigurationContext> loaded = store.loadLastKnownGood();
        assertTrue(loaded.isEmpty(), "Must return empty when no LKG is saved");
    }

    @Test
    void save_overwritesPrevious() throws Exception {
        ConfigFallbackStore store = new ConfigFallbackStore(tempDir);

        // Save twice -- second save must overwrite the first
        store.saveLastKnownGood(ConfigurationContext.DEFAULT);
        store.saveLastKnownGood(ConfigurationContext.DEFAULT);

        Optional<ConfigurationContext> loaded = store.loadLastKnownGood();
        assertTrue(loaded.isPresent(), "Must load the latest saved config");
    }
}
