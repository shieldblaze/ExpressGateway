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
package com.shieldblaze.expressgateway.controlplane.agent;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import lombok.extern.log4j.Log4j2;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Persists last-known-good configuration to local filesystem.
 * Nodes operate on LKG when disconnected from Control Plane.
 *
 * <p>Storage format: JSON file at configurable path.
 * Atomic write: write to temp file, then rename (POSIX atomic on same filesystem).</p>
 */
@Log4j2
public final class LKGStore {

    private final Path storagePath;
    private final ObjectMapper objectMapper;

    public LKGStore(Path storagePath) {
        this.storagePath = Objects.requireNonNull(storagePath, "storagePath");
        this.objectMapper = new ObjectMapper();
        this.objectMapper.findAndRegisterModules();
    }

    /**
     * Save config snapshot as LKG.
     *
     * @param resources all current config resources
     * @param revision  the global revision number
     * @throws IOException if the snapshot cannot be written
     */
    public void save(List<ConfigResource> resources, long revision) throws IOException {
        // Create parent directories if needed
        Path parent = storagePath.getParent();
        if (parent != null) {
            Files.createDirectories(parent);
        }

        var snapshot = Map.of("revision", revision, "resources", resources);
        Path tempFile = storagePath.resolveSibling(storagePath.getFileName() + ".tmp");
        Files.writeString(tempFile, objectMapper.writeValueAsString(snapshot),
                StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);
        Files.move(tempFile, storagePath, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        log.info("Saved LKG config at revision {}", revision);
    }

    /**
     * Load LKG config revision, if available.
     *
     * @return the revision, or -1 if no LKG exists
     */
    public long loadRevision() {
        if (!Files.exists(storagePath)) {
            return -1;
        }
        try {
            var node = objectMapper.readTree(storagePath.toFile());
            return node.get("revision").asLong(-1);
        } catch (Exception e) {
            log.warn("Failed to read LKG revision", e);
            return -1;
        }
    }

    /**
     * @return true if a LKG snapshot file exists on disk
     */
    public boolean exists() {
        return Files.exists(storagePath);
    }
}
