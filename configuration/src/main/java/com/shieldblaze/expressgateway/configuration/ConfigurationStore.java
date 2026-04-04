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
package com.shieldblaze.expressgateway.configuration;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.common.zookeeper.ZNodePath;
import com.shieldblaze.expressgateway.configuration.distributed.ConfigRolloutState;
import com.shieldblaze.expressgateway.configuration.distributed.ConfigStorageBackend;
import com.shieldblaze.expressgateway.configuration.distributed.DistributedConfigurationManager;
import io.netty.util.internal.StringUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.List;
import java.util.concurrent.CompletableFuture;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;
import static com.shieldblaze.expressgateway.common.zookeeper.CuratorUtils.createNew;
import static com.shieldblaze.expressgateway.common.zookeeper.CuratorUtils.deleteData;
import static java.lang.System.getProperty;

/**
 * {@link ConfigurationStore} provides API for storing, modification and retrieval of configurations.
 */
public final class ConfigurationStore {

    private static final Logger logger = LogManager.getLogger(ConfigurationStore.class);

    /**
     * Apply {@link Configuration} into ZooKeeper
     *
     * @param configuration {@link Configuration} to save
     * @throws Exception If an error occurs during operation
     */
    public static void applyConfiguration(Configuration<?> configuration) throws Exception {
        try {
            logger.info("Begin applying and saving configuration into ZooKeeper");
            configuration.assertValidated();

            String configurationJson = OBJECT_MAPPER.valueToTree(configuration).toString();
            createNew(Curator.getInstance(), of(configuration), configurationJson.getBytes());

            logger.info("Successfully applied and saved configuration into Zookeeper");
        } catch (Exception ex) {
            logger.fatal("Failed to apply configuration in ZooKeeper", ex);
            throw ex;
        }
    }

    /**
     * Remove {@link Configuration} from ZooKeeper
     *
     * @param configuration {@link Configuration} to remove
     * @throws Exception If an error occurs during operation
     */
    public static void removeConfiguration(Configuration<?> configuration) throws Exception {
        try {
            logger.info("Begin removing configuration from ZooKeeper");
            deleteData(Curator.getInstance(), of(configuration));
            logger.info("Successfully removed configuration from ZooKeeper");
        } catch (Exception ex) {
            logger.fatal("Failed to remove configuration from ZooKeeper", ex);
            throw ex;
        }
    }

    /**
     * Save {@link Configuration} into file.
     *
     * @param configuration {@link Configuration} to save
     * @throws IOException If an error occurs during operation
     */
    public static void save(Configuration<?> configuration) throws IOException {
        try {
            logger.info("Begin saving configuration into config directory");

            configuration.assertValidated();
            String json = OBJECT_MAPPER.writerWithDefaultPrettyPrinter().writeValueAsString(configuration);

            // Atomic write: TRUNCATE_EXISTING prevents stale trailing bytes if the new
            // content is shorter than the old file. CREATE ensures the file is made if absent.
            Files.writeString(Path.of(configDirPath(configuration.getClass())), json,
                    StandardOpenOption.CREATE, StandardOpenOption.WRITE, StandardOpenOption.TRUNCATE_EXISTING);

            logger.info("Successfully saved configuration into config directory");
        } catch (Exception ex) {
            logger.fatal("Failed to save configuration", ex);
            throw ex;
        }
    }

    /**
     * load {@link Configuration} from file
     *
     * @param clazz Class reference to load
     * @param <T>   Class
     * @return Class instance
     * @throws Exception If an error occurs during operation
     */
    public static <T> T load(Class<T> clazz) throws Exception {
        try {
            String data = Files.readString(Path.of(configDirPath(clazz)));
            return OBJECT_MAPPER.readValue(data, clazz);
        } catch (Exception ex) {
            logger.fatal("Failed to load configuration: {}", clazz, ex);
            throw ex;
        }
    }

    private static String configDirPath(Class<?> clazz) {
        // -> /etc/expressgateway/conf.d/default/CONFIG.json
        if (Environment.detectEnv() == Environment.PRODUCTION) {
            return getProperty("CONFIGURATION_DIRECTORY") + StringUtil.simpleClassName(clazz) + ".json";
        } else {
            return getProperty("CONFIGURATION_DIRECTORY", getProperty("java.io.tmpdir")) + File.separator + StringUtil.simpleClassName(clazz) + ".json";
        }
    }

    private static ZNodePath of(Configuration<?> configuration) {
        // Build ZNodePath for ZooKeeper
        return ZNodePath.create("ExpressGateway",               // ExpressGateway will be root
                Environment.detectEnv(),                                // Auto-detect environment
                getProperty(ExpressGateway.getInstance().clusterID()),  // Use Cluster ID as ID
                configuration.friendlyName());                          // Use Configuration name as component
    }

    // --- Distributed Configuration Methods ---

    private static volatile DistributedConfigurationManager distributedManager;

    /**
     * Initialize and start the distributed configuration manager.
     *
     * <p>This method enables distributed mode, allowing configuration to be managed
     * via ZooKeeper with versioning, leader election, and fallback support.
     * Calling this method does not affect the existing non-distributed API.</p>
     *
     * @param clusterId   The cluster identifier
     * @param environment The deployment environment
     * @throws Exception If an error occurs during initialization
     */
    public static synchronized void startDistributed(String clusterId, Environment environment) throws Exception {
        startDistributed(clusterId, environment, null);
    }

    /**
     * Initialize and start the distributed configuration manager with an explicit storage backend.
     *
     * <p>If {@code storageBackend} is {@code null}, the default Curator-backed backend is used.</p>
     *
     * @param clusterId      The cluster identifier
     * @param environment    The deployment environment
     * @param storageBackend The storage backend (may be {@code null} for default Curator backend)
     * @throws Exception If an error occurs during initialization
     */
    public static synchronized void startDistributed(String clusterId, Environment environment,
                                                     ConfigStorageBackend storageBackend) throws Exception {
        if (distributedManager != null) {
            throw new IllegalStateException("Distributed configuration manager is already started");
        }

        logger.info("Starting distributed configuration mode for cluster: {}", clusterId);
        DistributedConfigurationManager manager;
        if (storageBackend != null) {
            manager = new DistributedConfigurationManager(clusterId, environment, 3, storageBackend);
        } else {
            manager = new DistributedConfigurationManager(clusterId, environment);
        }
        manager.start();
        distributedManager = manager;
        logger.info("Distributed configuration mode started");
    }

    /**
     * Propose a new configuration context via the distributed manager.
     *
     * @param context The proposed {@link ConfigurationContext}
     * @return A {@link CompletableFuture} that completes with the final {@link ConfigRolloutState}
     * @throws IllegalStateException If distributed mode is not enabled or this node is not the leader
     */
    public static CompletableFuture<ConfigRolloutState> proposeDistributed(ConfigurationContext context) {
        DistributedConfigurationManager manager = distributedManager;
        if (manager == null) {
            throw new IllegalStateException("Distributed configuration manager is not started");
        }
        return manager.proposeConfig(context);
    }

    /**
     * Get the current configuration context from the distributed manager.
     *
     * @return The current {@link ConfigurationContext}
     * @throws IllegalStateException If distributed mode is not enabled
     */
    public static ConfigurationContext getDistributedConfig() {
        DistributedConfigurationManager manager = distributedManager;
        if (manager == null) {
            throw new IllegalStateException("Distributed configuration manager is not started");
        }
        return manager.getCurrentConfig();
    }

    /**
     * Rollback to a previous configuration version via the distributed manager.
     *
     * @param version The version number to roll back to
     * @throws Exception If an error occurs during the rollback
     * @throws IllegalStateException If distributed mode is not enabled or this node is not the leader
     */
    public static void rollbackDistributed(int version) throws Exception {
        DistributedConfigurationManager manager = distributedManager;
        if (manager == null) {
            throw new IllegalStateException("Distributed configuration manager is not started");
        }
        manager.rollback(version);
    }

    /**
     * List all available distributed configuration versions.
     *
     * @return A list of version numbers in ascending order
     * @throws Exception If an error occurs reading from ZooKeeper
     * @throws IllegalStateException If distributed mode is not enabled
     */
    public static List<Integer> listDistributedVersions() throws Exception {
        DistributedConfigurationManager manager = distributedManager;
        if (manager == null) {
            throw new IllegalStateException("Distributed configuration manager is not started");
        }
        return manager.listVersions();
    }

    /**
     * Check if distributed mode is enabled and this node is the leader.
     *
     * @return {@code true} if distributed mode is enabled and this node is the leader
     */
    public static boolean isDistributedLeader() {
        DistributedConfigurationManager manager = distributedManager;
        return manager != null && manager.isLeader();
    }

    /**
     * Check if distributed mode is currently enabled.
     *
     * @return {@code true} if distributed mode is enabled
     */
    public static boolean isDistributedMode() {
        return distributedManager != null;
    }

    /**
     * Shut down the distributed configuration manager.
     *
     * <p>This method does not affect the existing non-distributed API.</p>
     */
    public static void stopDistributed() {
        DistributedConfigurationManager manager = distributedManager;
        if (manager != null) {
            try {
                manager.close();
                logger.info("Distributed configuration manager stopped");
            } catch (IOException e) {
                logger.error("Error closing distributed configuration manager", e);
            } finally {
                distributedManager = null;
            }
        }
    }

    private ConfigurationStore() {
        // Prevent outside initialization
    }
}
