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

import com.fasterxml.jackson.databind.JsonNode;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.common.zookeeper.ZNodePath;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.Optional;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;

/**
 * Versioned configuration storage backed by a {@link ConfigStorageBackend}.
 *
 * <p>Key structure:</p>
 * <pre>
 * /ExpressGateway/{env}/{clusterId}/config/versions/v001
 * /ExpressGateway/{env}/{clusterId}/config/versions/v002
 * /ExpressGateway/{env}/{clusterId}/config/current  -> {"version": N}
 * </pre>
 *
 * <p>Version numbers are monotonically increasing integers, formatted as {@code v%03d}.</p>
 */
public final class ConfigVersionStore {

    private static final Logger logger = LogManager.getLogger(ConfigVersionStore.class);

    private static final String ROOT_PATH = "ExpressGateway";
    private static final String CONFIG_COMPONENT = "config";
    private static final String VERSIONS_COMPONENT = "versions";
    private static final String CURRENT_COMPONENT = "current";
    private static final String VERSION_FORMAT = "v%03d";

    private static final int MAX_VERSION_RETRIES = 5;

    private final String clusterId;
    private final Environment environment;
    private final ConfigStorageBackend storageBackend;

    /**
     * Creates a new {@link ConfigVersionStore}.
     *
     * @param clusterId      The cluster identifier
     * @param environment    The deployment environment
     * @param storageBackend The storage backend
     */
    public ConfigVersionStore(String clusterId, Environment environment, ConfigStorageBackend storageBackend) {
        this.clusterId = Objects.requireNonNull(clusterId, "clusterId");
        this.environment = Objects.requireNonNull(environment, "environment");
        this.storageBackend = Objects.requireNonNull(storageBackend, "storageBackend");
    }

    /**
     * Write a new versioned configuration.
     *
     * @param context The {@link ConfigurationContext} to persist
     * @return The version number assigned to this configuration
     * @throws Exception If an error occurs during the operation
     */
    public int writeVersion(ConfigurationContext context) throws Exception {
        Objects.requireNonNull(context, "ConfigurationContext");
        String json = OBJECT_MAPPER.writeValueAsString(context);

        for (int attempt = 0; attempt < MAX_VERSION_RETRIES; attempt++) {
            int nextVersion = determineNextVersion();
            String versionName = String.format(VERSION_FORMAT, nextVersion);

            String versionPath = ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT,
                    VERSIONS_COMPONENT + "/" + versionName).path();

            try {
                storageBackend.putIfAbsent(versionPath, json.getBytes(StandardCharsets.UTF_8));
                logger.info("Wrote configuration version {} at {}", versionName, versionPath);
                return nextVersion;
            } catch (ConfigStorageBackend.KeyExistsException e) {
                logger.warn("Version {} already exists (concurrent write detected), retrying (attempt {}/{})",
                        versionName, attempt + 1, MAX_VERSION_RETRIES);
            }
        }
        throw new IllegalStateException("Failed to write configuration version after " + MAX_VERSION_RETRIES + " attempts due to concurrent writes");
    }

    /**
     * Read a specific configuration version.
     *
     * @param version The version number to read
     * @return The {@link ConfigurationContext} stored at that version
     * @throws Exception If an error occurs or the version does not exist
     */
    public ConfigurationContext readVersion(int version) throws Exception {
        String versionName = String.format(VERSION_FORMAT, version);
        String versionPath = ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT,
                VERSIONS_COMPONENT + "/" + versionName).path();

        Optional<byte[]> data = storageBackend.get(versionPath);
        if (data.isEmpty()) {
            throw new IllegalArgumentException("Configuration version " + versionName + " does not exist");
        }

        String json = new String(data.get(), StandardCharsets.UTF_8);
        logger.debug("Read configuration version {} from storage", versionName);
        return OBJECT_MAPPER.readValue(json, ConfigurationContext.class);
    }

    /**
     * Get the current active configuration version number.
     *
     * @return The current version number, or -1 if no current version is set
     * @throws Exception If an error occurs during the operation
     */
    public int currentVersion() throws Exception {
        String currentPath = currentKeyPath();

        Optional<byte[]> data = storageBackend.get(currentPath);
        if (data.isEmpty()) {
            return -1;
        }

        String json = new String(data.get(), StandardCharsets.UTF_8);
        JsonNode node = OBJECT_MAPPER.readTree(json);
        return node.get("version").asInt();
    }

    /**
     * Set the current active configuration version pointer.
     *
     * @param version The version number to set as current
     * @throws Exception If an error occurs during the operation
     */
    public void setCurrent(int version) throws Exception {
        // Verify the target version exists
        String versionName = String.format(VERSION_FORMAT, version);
        String versionPath = ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT,
                VERSIONS_COMPONENT + "/" + versionName).path();

        if (!storageBackend.exists(versionPath)) {
            throw new IllegalArgumentException("Configuration version " + versionName + " does not exist");
        }

        String json = OBJECT_MAPPER.writeValueAsString(
                OBJECT_MAPPER.createObjectNode().put("version", version));

        storageBackend.put(currentKeyPath(), json.getBytes(StandardCharsets.UTF_8));
        logger.info("Set current configuration version to {}", versionName);
    }

    /**
     * List all available configuration version numbers.
     *
     * @return An unmodifiable list of version numbers in ascending order
     * @throws Exception If an error occurs during the operation
     */
    public List<Integer> listVersions() throws Exception {
        String versionsPath = ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT, VERSIONS_COMPONENT).path();

        if (!storageBackend.exists(versionsPath)) {
            return Collections.emptyList();
        }

        List<String> children = storageBackend.listChildren(versionsPath);
        List<Integer> versions = new ArrayList<>();

        for (String child : children) {
            try {
                // Parse "v001" -> 1
                versions.add(Integer.parseInt(child.substring(1)));
            } catch (NumberFormatException | StringIndexOutOfBoundsException e) {
                logger.warn("Ignoring unexpected child: {}", child);
            }
        }

        Collections.sort(versions);
        return Collections.unmodifiableList(versions);
    }

    private int determineNextVersion() throws Exception {
        String versionsPath = ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT, VERSIONS_COMPONENT).path();

        if (!storageBackend.exists(versionsPath)) {
            return 1;
        }

        List<String> children = storageBackend.listChildren(versionsPath);
        int max = 0;

        for (String child : children) {
            try {
                int v = Integer.parseInt(child.substring(1));
                if (v > max) {
                    max = v;
                }
            } catch (NumberFormatException | StringIndexOutOfBoundsException e) {
                // Ignore malformed children
            }
        }

        return max + 1;
    }

    private String currentKeyPath() {
        return ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT, CURRENT_COMPONENT).path();
    }
}
