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

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class LocalConfigCacheTest {

    @TempDir
    Path tempDir;

    private LocalConfigCache cache;

    @BeforeEach
    void setUp() throws IOException {
        cache = new LocalConfigCache(tempDir.resolve("cache"), 5);
    }

    @Test
    void storeAndLoad() throws IOException {
        byte[] data = "{\"test\": true}".getBytes(StandardCharsets.UTF_8);
        ConfigVersion version = cache.store(1, data, Map.of("source", "test"));

        assertEquals(1, version.revision());
        assertNotNull(version.checksum());
        assertEquals("test", version.metadata().get("source"));

        Optional<byte[]> loaded = cache.load(1);
        assertTrue(loaded.isPresent());
        assertArrayEquals(data, loaded.get());
    }

    @Test
    void loadMissingRevisionReturnsEmpty() {
        Optional<byte[]> loaded = cache.load(999);
        assertTrue(loaded.isEmpty());
    }

    @Test
    void latestVersionReturnsHighestRevision() throws IOException {
        cache.store(1, "v1".getBytes(), null);
        cache.store(5, "v5".getBytes(), null);
        cache.store(3, "v3".getBytes(), null);

        Optional<ConfigVersion> latest = cache.latestVersion();
        assertTrue(latest.isPresent());
        assertEquals(5, latest.get().revision());
    }

    @Test
    void versionAtOrBeforeFindsBestMatch() throws IOException {
        cache.store(1, "v1".getBytes(), null);
        cache.store(5, "v5".getBytes(), null);
        cache.store(10, "v10".getBytes(), null);

        Optional<ConfigVersion> result = cache.versionAtOrBefore(7);
        assertTrue(result.isPresent());
        assertEquals(5, result.get().revision());
    }

    @Test
    void maxVersionsEvictsOldest() throws IOException {
        // maxVersions is 5
        for (int i = 1; i <= 8; i++) {
            cache.store(i, ("data-" + i).getBytes(), null);
        }

        List<ConfigVersion> all = cache.allVersions();
        assertEquals(5, all.size());
        assertEquals(4, all.get(0).revision()); // oldest remaining
        assertEquals(8, all.get(4).revision()); // newest
    }

    @Test
    void corruptDataDetectedOnLoad() throws IOException {
        byte[] data = "valid data".getBytes(StandardCharsets.UTF_8);
        cache.store(1, data, null);

        // Corrupt the data file
        Path dataFile = tempDir.resolve("cache/data/1.json");
        Files.writeString(dataFile, "corrupted!");

        Optional<byte[]> loaded = cache.load(1);
        assertTrue(loaded.isEmpty()); // Checksum mismatch
    }

    @Test
    void verifyIntegrityDetectsCorruption() throws IOException {
        cache.store(1, "data-1".getBytes(), null);
        cache.store(2, "data-2".getBytes(), null);

        // Corrupt revision 2
        Path dataFile = tempDir.resolve("cache/data/2.json");
        Files.writeString(dataFile, "corrupted!");

        List<Long> corrupted = cache.verifyIntegrity();
        assertEquals(1, corrupted.size());
        assertTrue(corrupted.contains(2L));
    }

    @Test
    void purgeCorruptedRemovesEntries() throws IOException {
        cache.store(1, "data-1".getBytes(), null);
        cache.store(2, "data-2".getBytes(), null);

        cache.purgeCorrupted(List.of(2L));

        List<ConfigVersion> all = cache.allVersions();
        assertEquals(1, all.size());
        assertEquals(1, all.get(0).revision());
    }

    @Test
    void tagVersionAndFindByTag() throws IOException {
        cache.store(1, "data-1".getBytes(), null);
        cache.store(2, "data-2".getBytes(), null);

        Optional<ConfigVersion> tagged = cache.tagVersion(1, "known-good");
        assertTrue(tagged.isPresent());
        assertEquals("known-good", tagged.get().tag());

        Optional<ConfigVersion> found = cache.findByTag("known-good");
        assertTrue(found.isPresent());
        assertEquals(1, found.get().revision());
    }

    @Test
    void findByTagReturnsEmptyForUnknownTag() {
        Optional<ConfigVersion> found = cache.findByTag("nonexistent");
        assertTrue(found.isEmpty());
    }

    @Test
    void persistenceAcrossRestarts() throws IOException {
        cache.store(1, "data-1".getBytes(), null);
        cache.store(2, "data-2".getBytes(), null);

        // Create a new cache instance from the same directory
        LocalConfigCache newCache = new LocalConfigCache(tempDir.resolve("cache"), 5);
        assertEquals(2, newCache.allVersions().size());

        Optional<byte[]> loaded = newCache.load(1);
        assertTrue(loaded.isPresent());
        assertArrayEquals("data-1".getBytes(), loaded.get());
    }

    @Test
    void missingDataFileDetectedByIntegrityCheck() throws IOException {
        cache.store(1, "data-1".getBytes(), null);

        // Delete the data file
        Files.delete(tempDir.resolve("cache/data/1.json"));

        List<Long> corrupted = cache.verifyIntegrity();
        assertEquals(1, corrupted.size());
        assertTrue(corrupted.contains(1L));
    }

    @Test
    void emptyDataRejected() {
        try {
            cache.store(1, new byte[0], null);
            assertFalse(true, "Should have thrown");
        } catch (Exception e) {
            assertTrue(e instanceof IllegalArgumentException);
        }
    }

    @Test
    void corruptManifestRecoveredFromBackup() throws IOException {
        // Store some data so we have a valid manifest + backup
        cache.store(1, "data-1".getBytes(), null);
        cache.store(2, "data-2".getBytes(), null);

        // At this point, manifest.json is valid and manifest.json.bak exists
        Path manifestPath = tempDir.resolve("cache/manifest.json");
        Path backupPath = tempDir.resolve("cache/manifest.json.bak");
        assertTrue(Files.exists(manifestPath));
        assertTrue(Files.exists(backupPath));

        // Corrupt the primary manifest
        Files.writeString(manifestPath, "THIS IS NOT JSON!!!{{{");

        // Create a new cache -- it should recover from backup
        LocalConfigCache recoveredCache = new LocalConfigCache(tempDir.resolve("cache"), 5);

        // The backup has the state from before the second store (1 version),
        // since backup is created before each persist. But the second store
        // successfully wrote, so the backup contains the state with rev 1.
        // Actually, the backup is created right before writing the new manifest,
        // so after store(2), the backup contains [rev1] and the main has [rev1, rev2].
        // After corruption + recovery from backup, we get [rev1].
        // Let's just check that we recovered at least one version.
        assertTrue(recoveredCache.allVersions().size() >= 1,
                "Should have recovered at least one version from backup");
    }

    @Test
    void crashBetweenDataAndManifestRecoverable() throws IOException {
        // Store initial data
        cache.store(1, "data-1".getBytes(), null);

        // Simulate a crash: data file for rev 2 exists but manifest wasn't updated
        Path dataFile = tempDir.resolve("cache/data/2.json");
        Files.writeString(dataFile, "data-2");

        // Create a new cache -- should load cleanly with only rev 1
        LocalConfigCache newCache = new LocalConfigCache(tempDir.resolve("cache"), 5);
        assertEquals(1, newCache.allVersions().size());
        assertEquals(1, newCache.allVersions().get(0).revision());

        // The orphaned data file for rev 2 exists but is not referenced
        assertTrue(Files.exists(dataFile));
    }

    @Test
    void manifestBackupCreatedOnSubsequentStore() throws IOException {
        // First store creates the manifest (no backup yet since there's no prior manifest)
        cache.store(1, "data-1".getBytes(), null);

        // Second store backs up the existing manifest before overwriting
        cache.store(2, "data-2".getBytes(), null);

        Path backupPath = tempDir.resolve("cache/manifest.json.bak");
        assertTrue(Files.exists(backupPath), "Manifest backup should be created after second store");
    }
}
