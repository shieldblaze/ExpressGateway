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

import com.shieldblaze.expressgateway.common.MongoDB;
import com.shieldblaze.expressgateway.common.curator.Curator;
import com.shieldblaze.expressgateway.common.curator.Environment;
import com.shieldblaze.expressgateway.common.curator.ZNodePath;
import dev.morphia.query.experimental.filters.Filters;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CLUSTER_ID;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.createNew;
import static com.shieldblaze.expressgateway.common.curator.CuratorUtils.deleteData;

/**
 * {@link ConfigurationStore} is responsible for marshalling/unmarshalling of
 * {@link Configuration} into Json.
 */
public final class ConfigurationStore {

    private static final Logger logger = LogManager.getLogger(ConfigurationStore.class);

    /**
     * Save {@link Configuration} into ZooKeeper
     *
     * @param configuration {@link Configuration} to save
     * @throws Exception If an error occurs during operation
     */
    public static void applyConfiguration(Configuration<?> configuration) throws Exception {
        configuration.assertValidated();
        logger.info("Begin applying and saving configuration into ZooKeeper");

        String configurationJson = OBJECT_MAPPER.valueToTree(configuration).toString();
        createNew(Curator.getInstance(), buildZNodePath(configuration), configurationJson.getBytes());

        logger.info("Successfully applied and saved configuration into Zookeeper");
    }

    /**
     * Remove {@link Configuration} from ZooKeeper
     *
     * @param configuration {@link Configuration} to remove
     * @throws Exception If an error occurs during operation
     */
    public static void removeConfiguration(Configuration<?> configuration) throws Exception {
        deleteData(Curator.getInstance(), buildZNodePath(configuration));
    }

    /**
     * Save {@link Configuration} into MongoDB database
     *
     * @param configuration {@link Configuration} to save
     * @throws Exception If an error occurs during operation
     */
    public static void save(Configuration<?> configuration) throws Exception {
        configuration.assertValidated();
        logger.info("Begin saving configuration into MongoDB database");

        // Save Configuration in MongoDB database also
        MongoDB.getInstance().save(configuration);

        logger.info("Successfully saved configuration into MongoDB database");
    }

    /**
     * load {@link Configuration} from MongoDB database
     *
     * @param id    Configuration ID
     * @param clazz Class reference to load
     * @param <T>   Class
     * @return Class instance
     * @throws Exception If an error occurs during operation
     */
    public static <T> T load(String id, Class<T> clazz) throws Exception {
        return MongoDB.getInstance().find(clazz)
                .filter(Filters.eq("_id", id))
                .first();
    }

    /**
     * Delete configuration from MongoDB database
     *
     * @param configuration {@link Configuration} to delete
     * @return {@code true} if deletion was successful else {@code false}
     */
    public static boolean delete(Configuration<?> configuration) throws Exception {
        return MongoDB.getInstance().delete(configuration).wasAcknowledged();
    }

    private static ZNodePath buildZNodePath(Configuration<?> configuration) {
        // Build ZNodePath for ZooKeeper
        return ZNodePath.create("ExpressGateway", // ExpressGateway will be root
                Environment.detectEnv(),        // Auto-detect environment
                System.getProperty(CLUSTER_ID), // Use Cluster ID as ID
                configuration.friendlyName());  // Use Configuration name as component
    }

    private ConfigurationStore() {
        // Prevent outside initialization
    }
}
