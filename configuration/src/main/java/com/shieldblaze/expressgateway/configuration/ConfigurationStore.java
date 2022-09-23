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

import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.common.MongoDB;
import dev.morphia.query.experimental.filters.Filters;
import io.netty.util.internal.StringUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * {@link ConfigurationStore} is responsible for marshalling/unmarshalling of
 * {@link Configuration} into Json.
 */
public final class ConfigurationStore {

    private static final Logger logger = LogManager.getLogger(ConfigurationStore.class);
    private static final ObjectMapper OBJECT_MAPPER = new ObjectMapper();

    private ConfigurationStore() {
        // Prevent outside initialization
    }

    /**
     * Marshal and save {@link Configuration} into Json
     *
     * @param configuration {@link Configuration} to save
     * @throws IOException If an error occurs during operation
     */
    public static void save(Configuration<?> configuration) throws IOException {
        MongoDB.getInstance().save(configuration);
    }

    /**
     * Unmarshal and load Json into {@link Configuration}
     *
     * @param id    Configuration ID
     * @param clazz Class reference to load
     * @param <T>   Class
     * @return Class instance
     * @throws IOException If an error occurs during operation
     */
    public static <T> T load(String id, Class<T> clazz) throws IOException {
        return MongoDB.getInstance().find(clazz)
                .filter(Filters.eq("_id", id))
                .first();
    }

    /**
     * Delete configuration file
     *
     * @param id    Configuration ID
     * @param clazz Class reference to delete
     * @param <T>   Class
     * @return {@code true} if deletion was successful else {@code false}
     */
    public static <T> boolean delete(String id, Class<T> clazz) {
        String fileName = new StringBuilder()
                .append("/etc/expressgateway").append("/")                 // -> /etc/expressgateway
                .append("conf.d").append("/")                              // -> /etc/expressgateway/conf.d/
                .append(id).append("/")                                    // -> /etc/expressgateway/conf.d/$PROFILE_NAME/
                .append(StringUtil.simpleClassName(clazz)).append(".json") // -> /etc/expressgateway/conf.d/$PROFILE_NAME/$CONFIG.json
                .toString();

        try {
            return new File(fileName).delete();
        } catch (Exception ex) {
            return false;
        }
    }
}
