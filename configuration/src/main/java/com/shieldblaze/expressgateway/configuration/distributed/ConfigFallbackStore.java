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
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.Objects;
import java.util.Optional;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;

/**
 * Local file-based last-known-good configuration cache.
 *
 * <p>Provides a fallback mechanism that persists the last successfully applied
 * {@link ConfigurationContext} to local disk. This allows the node to recover
 * its configuration when ZooKeeper is unavailable.</p>
 */
public final class ConfigFallbackStore {

    private static final Logger logger = LogManager.getLogger(ConfigFallbackStore.class);

    private static final String DEFAULT_DIRECTORY = "/tmp/expressgateway-lkg/";
    private static final String LKG_FILE_NAME = "last-known-good-config.json";

    private final Path lkgFilePath;

    /**
     * Creates a new {@link ConfigFallbackStore} with the default storage directory.
     */
    public ConfigFallbackStore() {
        this(Path.of(DEFAULT_DIRECTORY));
    }

    /**
     * Creates a new {@link ConfigFallbackStore} with a custom storage directory.
     *
     * @param directory The directory where the last-known-good configuration file will be stored
     */
    public ConfigFallbackStore(Path directory) {
        Objects.requireNonNull(directory, "directory");
        this.lkgFilePath = directory.resolve(LKG_FILE_NAME);
    }

    /**
     * Save the given configuration as the last-known-good configuration.
     *
     * @param context The {@link ConfigurationContext} to persist
     * @throws IOException If an error occurs during file I/O
     */
    public void saveLastKnownGood(ConfigurationContext context) throws IOException {
        Objects.requireNonNull(context, "ConfigurationContext");

        try {
            // Ensure the parent directory exists
            Path parentDir = lkgFilePath.getParent();
            if (parentDir != null) {
                Files.createDirectories(parentDir);
            }

            String json = OBJECT_MAPPER.writerWithDefaultPrettyPrinter().writeValueAsString(context);

            // LKG-1 FIX: Atomic write via temp file + fsync + rename.
            // A crash during write leaves only the temp file; the original LKG file
            // remains intact. This matches the pattern used by the agent's LKGStore.
            Path tempFile = lkgFilePath.resolveSibling(lkgFilePath.getFileName() + ".tmp");
            Files.writeString(tempFile, json,
                    StandardOpenOption.CREATE, StandardOpenOption.WRITE, StandardOpenOption.TRUNCATE_EXISTING,
                    StandardOpenOption.SYNC);
            Files.move(tempFile, lkgFilePath, java.nio.file.StandardCopyOption.ATOMIC_MOVE,
                    java.nio.file.StandardCopyOption.REPLACE_EXISTING);

            logger.info("Saved last-known-good configuration to {}", lkgFilePath);
        } catch (IOException e) {
            logger.error("Failed to save last-known-good configuration to {}", lkgFilePath, e);
            throw e;
        }
    }

    /**
     * Load the last-known-good configuration from disk.
     *
     * @return An {@link Optional} containing the {@link ConfigurationContext} if available,
     *         or empty if no last-known-good configuration exists or cannot be read
     */
    public Optional<ConfigurationContext> loadLastKnownGood() {
        try {
            if (!Files.exists(lkgFilePath)) {
                logger.info("No last-known-good configuration file found at {}", lkgFilePath);
                return Optional.empty();
            }

            String json = Files.readString(lkgFilePath);
            ConfigurationContext context = OBJECT_MAPPER.readValue(json, ConfigurationContext.class);

            logger.info("Loaded last-known-good configuration from {}", lkgFilePath);
            return Optional.of(context);
        } catch (Exception e) {
            logger.error("Failed to load last-known-good configuration from {}", lkgFilePath, e);
            return Optional.empty();
        }
    }
}
