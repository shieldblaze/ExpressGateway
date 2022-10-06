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

import com.shieldblaze.expressgateway.common.curator.Curator;
import com.shieldblaze.expressgateway.common.curator.Environment;
import com.shieldblaze.expressgateway.common.curator.ZNodePath;
import io.netty.util.internal.StringUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CLUSTER_ID;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CONFIGURATION_DIRECTORY;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.createNew;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.deleteData;
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
            createNew(Curator.getInstance(), buildZNodePath(configuration), configurationJson.getBytes());

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
            logger.info("Being removing configuration from ZooKeeper");
            deleteData(Curator.getInstance(), buildZNodePath(configuration));
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

            try (FileWriter writer = new FileWriter(configDirPath(configuration.getClass()), false)) {
                writer.write(json);
            }

            // Write configuration into file
            Files.writeString(Path.of(configDirPath(configuration.getClass())), json, StandardOpenOption.CREATE, StandardOpenOption.WRITE);

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
            return getProperty(CONFIGURATION_DIRECTORY.name()) + StringUtil.simpleClassName(clazz) + ".json";
        } else {
            return getProperty(CONFIGURATION_DIRECTORY.name(), getProperty("java.io.tmpdir")) + StringUtil.simpleClassName(clazz) + ".json";
        }
    }

    private static ZNodePath buildZNodePath(Configuration<?> configuration) {
        // Build ZNodePath for ZooKeeper
        return ZNodePath.create("ExpressGateway", // ExpressGateway will be root
                Environment.detectEnv(),                  // Auto-detect environment
                getProperty(CLUSTER_ID.name()),           // Use Cluster ID as ID
                configuration.friendlyName());            // Use Configuration name as component
    }

    private ConfigurationStore() {
        // Prevent outside initialization
    }
}
