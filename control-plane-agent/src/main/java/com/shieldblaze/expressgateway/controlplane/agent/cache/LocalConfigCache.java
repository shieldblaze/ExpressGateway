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

import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;
import lombok.extern.slf4j.Slf4j;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.TreeMap;
import java.util.concurrent.locks.ReadWriteLock;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * File-based local config cache with version history, checksum integrity, and rollback support.
 *
 * <p>Each config version is stored as a separate file in the cache directory, named by revision.
 * A manifest file tracks all known versions and their checksums. All writes use crash-safe
 * atomic file operations (write-to-temp + rename).</p>
 *
 * <h3>Directory layout</h3>
 * <pre>
 *   {cacheDir}/
 *     manifest.json          -- version index (revision -> ConfigVersion)
 *     manifest.json.bak      -- backup of the previous manifest
 *     data/
 *       {revision}.json      -- serialized config data for that revision
 * </pre>
 *
 * <h3>Safety guarantees</h3>
 * <ul>
 *   <li>Atomic writes: temp file + rename prevents partial writes on crash</li>
 *   <li>Checksum verification: SHA-256 of data is checked on every read</li>
 *   <li>Corruption detection: mismatched checksums cause automatic fallback</li>
 *   <li>Partial update rejection: all-or-nothing for manifest+data writes</li>
 *   <li>Manifest backup: manifest.json.bak is maintained for crash recovery</li>
 * </ul>
 *
 * <p>Thread safety: all public methods are guarded by a {@link ReadWriteLock}.</p>
 */
@Slf4j
public final class LocalConfigCache {

    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final String MANIFEST_FILE = "manifest.json";
    private static final String MANIFEST_BACKUP = "manifest.json.bak";
    private static final String DATA_DIR = "data";

    static {
        MAPPER.findAndRegisterModules();
    }

    private final Path cacheDir;
    private final int maxVersions;
    private final ReadWriteLock lock = new ReentrantReadWriteLock();
    // TreeMap for sorted access by revision
    private final TreeMap<Long, ConfigVersion> versions = new TreeMap<>();

    /**
     * Creates a new local config cache.
     *
     * @param cacheDir    the root directory for the cache
     * @param maxVersions the maximum number of versions to retain (must be >= 1)
     * @throws IOException if the cache directory cannot be created
     */
    public LocalConfigCache(Path cacheDir, int maxVersions) throws IOException {
        this.cacheDir = Objects.requireNonNull(cacheDir, "cacheDir");
        if (maxVersions < 1) {
            throw new IllegalArgumentException("maxVersions must be >= 1, got: " + maxVersions);
        }
        this.maxVersions = maxVersions;

        // Create directory structure
        Files.createDirectories(cacheDir.resolve(DATA_DIR));

        // Load existing manifest
        loadManifest();
        log.info("LocalConfigCache initialized at {}, {} versions loaded, maxVersions={}",
                cacheDir, versions.size(), maxVersions);
    }

    /**
     * Stores a new config version with the given data.
     *
     * <p>The operation is atomic: data file is written first, then the manifest is
     * updated via temp+rename. If data write fails, the data file is cleaned up.
     * If manifest write fails, both manifest temp and data file are cleaned up.</p>
     *
     * @param revision the config revision number
     * @param data     the serialized config data
     * @param metadata optional metadata for this version
     * @return the created {@link ConfigVersion}
     * @throws IOException if the write fails
     */
    public ConfigVersion store(long revision, byte[] data, Map<String, String> metadata) throws IOException {
        Objects.requireNonNull(data, "data");
        if (data.length == 0) {
            throw new IllegalArgumentException("data must not be empty");
        }

        String checksum = sha256(data);
        ConfigVersion version = new ConfigVersion(revision, Instant.now(), checksum, metadata);

        lock.writeLock().lock();
        try {
            // Write data file atomically
            Path dataFile = dataFilePath(revision);
            try {
                atomicWrite(dataFile, data);
            } catch (IOException e) {
                // Clean up the data file on failure
                Files.deleteIfExists(dataFile);
                Path dataTemp = dataFile.resolveSibling(dataFile.getFileName() + ".tmp");
                Files.deleteIfExists(dataTemp);
                throw e;
            }

            // Update in-memory manifest and persist
            versions.put(revision, version);

            // Evict old versions if over limit
            while (versions.size() > maxVersions) {
                Map.Entry<Long, ConfigVersion> oldest = versions.firstEntry();
                if (oldest != null) {
                    versions.pollFirstEntry();
                    Path oldFile = dataFilePath(oldest.getKey());
                    Files.deleteIfExists(oldFile);
                    log.debug("Evicted old config version: revision={}", oldest.getKey());
                }
            }

            // Persist manifest atomically (write to temp first, then rename)
            try {
                persistManifest();
            } catch (IOException e) {
                // Manifest write failed: clean up the data file and rollback in-memory state
                versions.remove(revision);
                Files.deleteIfExists(dataFile);
                Path manifestTemp = cacheDir.resolve(MANIFEST_FILE + ".tmp");
                Files.deleteIfExists(manifestTemp);
                throw e;
            }

            log.info("Stored config version: revision={}, checksum={}, size={}",
                    revision, checksum, data.length);
            return version;
        } catch (IOException e) {
            // Rollback: remove from in-memory manifest on failure
            versions.remove(revision);
            throw e;
        } finally {
            lock.writeLock().unlock();
        }
    }

    /**
     * Loads the config data for the given revision after verifying its checksum.
     *
     * @param revision the revision to load
     * @return the config data, or empty if the revision is not found or corrupt
     */
    public Optional<byte[]> load(long revision) {
        lock.readLock().lock();
        try {
            ConfigVersion version = versions.get(revision);
            if (version == null) {
                return Optional.empty();
            }

            Path dataFile = dataFilePath(revision);
            if (!Files.exists(dataFile)) {
                log.warn("Data file missing for revision {}", revision);
                return Optional.empty();
            }

            byte[] data = Files.readAllBytes(dataFile);
            String actualChecksum = sha256(data);

            if (!actualChecksum.equals(version.checksum())) {
                log.error("Checksum mismatch for revision {}: expected={}, actual={}",
                        revision, version.checksum(), actualChecksum);
                return Optional.empty();
            }

            return Optional.of(data);
        } catch (IOException e) {
            log.error("Failed to load config data for revision {}", revision, e);
            return Optional.empty();
        } finally {
            lock.readLock().unlock();
        }
    }

    /**
     * Returns the latest known config version, if any.
     */
    public Optional<ConfigVersion> latestVersion() {
        lock.readLock().lock();
        try {
            if (versions.isEmpty()) {
                return Optional.empty();
            }
            return Optional.of(versions.lastEntry().getValue());
        } finally {
            lock.readLock().unlock();
        }
    }

    /**
     * Returns the version at or before the given revision, if any.
     * Useful for finding the closest rollback point.
     *
     * @param revision the target revision
     * @return the version at or before the given revision
     */
    public Optional<ConfigVersion> versionAtOrBefore(long revision) {
        lock.readLock().lock();
        try {
            Map.Entry<Long, ConfigVersion> entry = versions.floorEntry(revision);
            return Optional.ofNullable(entry).map(Map.Entry::getValue);
        } finally {
            lock.readLock().unlock();
        }
    }

    /**
     * Returns all stored versions in ascending revision order.
     */
    public List<ConfigVersion> allVersions() {
        lock.readLock().lock();
        try {
            return Collections.unmodifiableList(new ArrayList<>(versions.values()));
        } finally {
            lock.readLock().unlock();
        }
    }

    /**
     * Tags a specific version for easy identification (e.g., rollback point).
     *
     * @param revision the revision to tag
     * @param tag      the tag name
     * @return the updated version, or empty if the revision was not found
     * @throws IOException if the manifest update fails
     */
    public Optional<ConfigVersion> tagVersion(long revision, String tag) throws IOException {
        Objects.requireNonNull(tag, "tag");
        lock.writeLock().lock();
        try {
            ConfigVersion existing = versions.get(revision);
            if (existing == null) {
                return Optional.empty();
            }
            ConfigVersion tagged = existing.withTag(tag);
            versions.put(revision, tagged);
            persistManifest();
            log.info("Tagged config version: revision={}, tag={}", revision, tag);
            return Optional.of(tagged);
        } finally {
            lock.writeLock().unlock();
        }
    }

    /**
     * Finds a version by its tag.
     *
     * @param tag the tag to search for
     * @return the version with the matching tag, or empty if not found
     */
    public Optional<ConfigVersion> findByTag(String tag) {
        Objects.requireNonNull(tag, "tag");
        lock.readLock().lock();
        try {
            return versions.values().stream()
                    .filter(v -> tag.equals(v.tag()))
                    .findFirst();
        } finally {
            lock.readLock().unlock();
        }
    }

    /**
     * Verifies the integrity of all stored versions by checking their checksums.
     *
     * @return a list of revisions that have corrupted data
     */
    public List<Long> verifyIntegrity() {
        lock.readLock().lock();
        try {
            List<Long> corrupted = new ArrayList<>();
            for (Map.Entry<Long, ConfigVersion> entry : versions.entrySet()) {
                long revision = entry.getKey();
                ConfigVersion version = entry.getValue();
                Path dataFile = dataFilePath(revision);

                try {
                    if (!Files.exists(dataFile)) {
                        corrupted.add(revision);
                        continue;
                    }
                    byte[] data = Files.readAllBytes(dataFile);
                    String actualChecksum = sha256(data);
                    if (!actualChecksum.equals(version.checksum())) {
                        corrupted.add(revision);
                    }
                } catch (IOException e) {
                    corrupted.add(revision);
                }
            }
            if (!corrupted.isEmpty()) {
                log.warn("Integrity check found {} corrupted versions: {}", corrupted.size(), corrupted);
            }
            return Collections.unmodifiableList(corrupted);
        } finally {
            lock.readLock().unlock();
        }
    }

    /**
     * Removes corrupted versions from the cache.
     *
     * @param corruptedRevisions the revisions to remove
     * @throws IOException if the manifest update fails
     */
    public void purgeCorrupted(List<Long> corruptedRevisions) throws IOException {
        Objects.requireNonNull(corruptedRevisions, "corruptedRevisions");
        lock.writeLock().lock();
        try {
            for (long revision : corruptedRevisions) {
                versions.remove(revision);
                Files.deleteIfExists(dataFilePath(revision));
                log.info("Purged corrupted config version: revision={}", revision);
            }
            persistManifest();
        } finally {
            lock.writeLock().unlock();
        }
    }

    // ---- Internal ----

    private void loadManifest() {
        Path manifestPath = cacheDir.resolve(MANIFEST_FILE);
        if (!Files.exists(manifestPath)) {
            // Try to recover from backup
            Path backupPath = cacheDir.resolve(MANIFEST_BACKUP);
            if (Files.exists(backupPath)) {
                log.warn("Primary manifest missing, attempting recovery from backup: {}", backupPath);
                try {
                    List<ConfigVersion> loaded = MAPPER.readValue(backupPath.toFile(),
                            new TypeReference<List<ConfigVersion>>() {});
                    for (ConfigVersion v : loaded) {
                        versions.put(v.revision(), v);
                    }
                    // Restore the primary manifest from backup
                    Files.copy(backupPath, manifestPath, StandardCopyOption.REPLACE_EXISTING);
                    log.info("Successfully recovered manifest from backup ({} versions)", versions.size());
                    return;
                } catch (IOException e) {
                    log.error("Failed to recover manifest from backup, starting fresh", e);
                }
            }
            return;
        }
        try {
            List<ConfigVersion> loaded = MAPPER.readValue(manifestPath.toFile(),
                    new TypeReference<List<ConfigVersion>>() {});
            for (ConfigVersion v : loaded) {
                versions.put(v.revision(), v);
            }
        } catch (IOException e) {
            log.warn("Failed to load manifest from {}, attempting recovery from backup", manifestPath, e);
            // Attempt recovery from backup
            Path backupPath = cacheDir.resolve(MANIFEST_BACKUP);
            if (Files.exists(backupPath)) {
                try {
                    List<ConfigVersion> loaded = MAPPER.readValue(backupPath.toFile(),
                            new TypeReference<List<ConfigVersion>>() {});
                    for (ConfigVersion v : loaded) {
                        versions.put(v.revision(), v);
                    }
                    log.info("Recovered manifest from backup ({} versions)", versions.size());
                    return;
                } catch (IOException ex) {
                    log.error("Failed to recover manifest from backup, starting fresh", ex);
                }
            }
        }
    }

    private void persistManifest() throws IOException {
        Path manifestPath = cacheDir.resolve(MANIFEST_FILE);
        Path backupPath = cacheDir.resolve(MANIFEST_BACKUP);

        // Back up the existing manifest before overwriting
        if (Files.exists(manifestPath)) {
            Files.copy(manifestPath, backupPath, StandardCopyOption.REPLACE_EXISTING);
        }

        byte[] data = MAPPER.writeValueAsBytes(new ArrayList<>(versions.values()));
        atomicWrite(manifestPath, data);
    }

    private void atomicWrite(Path target, byte[] data) throws IOException {
        Path temp = target.resolveSibling(target.getFileName() + ".tmp");
        Files.write(temp, data, StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);
        Files.move(temp, target, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
    }

    private Path dataFilePath(long revision) {
        return cacheDir.resolve(DATA_DIR).resolve(revision + ".json");
    }

    static String sha256(byte[] data) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            byte[] hash = digest.digest(data);
            return HexFormat.of().formatHex(hash);
        } catch (NoSuchAlgorithmException e) {
            // SHA-256 is required by the JCE specification; cannot happen
            throw new AssertionError("SHA-256 not available", e);
        }
    }
}
