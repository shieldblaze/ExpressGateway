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
import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import io.netty.util.internal.StringUtil;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * {@link Store} is responsible for marshalling/unmarshalling of
 * {@link Configuration} into Json.
 */
final class Store {

    private static final ObjectMapper OBJECT_MAPPER = new ObjectMapper();

    /**
     * Marshal and save {@link Configuration} into Json
     *
     * @param profile       {@link Profile} instance
     * @param configuration {@link Configuration} to save
     * @throws IOException If an error occurs during operation
     */
    static void save(Profile profile, Configuration<?> configuration) throws IOException {
        String json = OBJECT_MAPPER.writerWithDefaultPrettyPrinter()
                .writeValueAsString(configuration);

        StringBuilder fileName = new StringBuilder()
                .append(getDir()).append(File.separator)
                .append("conf.d").append(File.separator)
                .append(profile.name()).append(File.separator);

        // Ensure directory exists so we can create file
        new File(fileName.toString()).mkdirs();
        fileName.append(StringUtil.simpleClassName(configuration)).append(".json");

        try (FileWriter writer = new FileWriter(fileName.toString(), false)) {
            writer.write(json);
        }
    }

    /**
     * Unmarshal and load Json into {@link Configuration}
     *
     * @param profile {@link Profile} instance
     * @param clazz   Class reference to load
     * @param <T>     Class
     * @return Class instance
     * @throws IOException If an error occurs during operation
     */
    static <T> T load(Profile profile, Class<T> clazz) throws IOException {
        String fileName = new StringBuilder()
                .append(getDir()).append(File.separator)
                .append("conf.d").append(File.separator)
                .append(profile.name()).append(File.separator)
                .append(StringUtil.simpleClassName(clazz)).append(".json")
                .toString();

        String json = Files.readString(Path.of(fileName));
        return OBJECT_MAPPER.readValue(json, clazz);
    }

    private static String getDir() {
        return SystemPropertyUtil.getPropertyOrEnv("egw.dir", System.getProperty("user.dir"));
    }

    private Store() {
        // Prevent outside initialization
    }
}
