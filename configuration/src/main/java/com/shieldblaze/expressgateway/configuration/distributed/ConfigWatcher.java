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

import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.common.zookeeper.ZNodePath;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.nio.charset.StandardCharsets;
import java.util.Objects;
import java.util.function.Consumer;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;

/**
 * Configuration change listener backed by a {@link ConfigStorageBackend}.
 *
 * <p>Watches the {@code /ExpressGateway/{env}/{clusterId}/config/current} key for changes.
 * When the watched key data changes, reads the new version, deserializes, validates,
 * and invokes the registered {@link Consumer} callback.</p>
 */
public final class ConfigWatcher implements Closeable {

    private static final Logger logger = LogManager.getLogger(ConfigWatcher.class);

    private static final String ROOT_PATH = "ExpressGateway";
    private static final String CONFIG_COMPONENT = "config";
    private static final String CURRENT_COMPONENT = "current";

    private final String clusterId;
    private final Environment environment;
    private final Consumer<ConfigurationContext> callback;
    private final ConfigVersionStore versionStore;
    private final ConfigStorageBackend storageBackend;

    private volatile Closeable watchHandle;

    /**
     * Creates a new {@link ConfigWatcher}.
     *
     * @param clusterId      The cluster identifier
     * @param environment    The deployment environment
     * @param versionStore   The {@link ConfigVersionStore} used to read versioned configs
     * @param callback       The callback invoked when a new configuration is detected
     * @param storageBackend The storage backend for watching
     */
    public ConfigWatcher(String clusterId, Environment environment,
                         ConfigVersionStore versionStore, Consumer<ConfigurationContext> callback,
                         ConfigStorageBackend storageBackend) {
        this.clusterId = Objects.requireNonNull(clusterId, "clusterId");
        this.environment = Objects.requireNonNull(environment, "environment");
        this.versionStore = Objects.requireNonNull(versionStore, "versionStore");
        this.callback = Objects.requireNonNull(callback, "callback");
        this.storageBackend = Objects.requireNonNull(storageBackend, "storageBackend");
    }

    /**
     * Start watching for configuration changes.
     *
     * @throws Exception If an error occurs during initialization
     */
    public void start() throws Exception {
        String watchPath = ZNodePath.create(ROOT_PATH, environment, clusterId, CONFIG_COMPONENT, CURRENT_COMPONENT).path();

        watchHandle = storageBackend.watch(watchPath, this::handleConfigChange);

        logger.info("Started ConfigWatcher on path: {}", watchPath);
    }

    private void handleConfigChange(byte[] data) {
        try {
            if (data == null || data.length == 0) {
                logger.warn("Received empty data for configuration current pointer, ignoring");
                return;
            }

            String json = new String(data, StandardCharsets.UTF_8);
            int version = OBJECT_MAPPER.readTree(json).get("version").asInt();

            logger.info("Loading configuration version {}", version);
            ConfigurationContext context = versionStore.readVersion(version);

            callback.accept(context);
            logger.info("Successfully applied configuration version {}", version);
        } catch (Exception e) {
            logger.error("Failed to process configuration change, continuing with current config", e);
        }
    }

    @Override
    public void close() {
        Closeable handle = this.watchHandle;
        if (handle != null) {
            try {
                handle.close();
            } catch (Exception e) {
                logger.warn("Error closing watch handle", e);
            }
            logger.info("Closed ConfigWatcher");
        }
    }
}
