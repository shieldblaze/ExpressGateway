/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;

import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

public final class ConfigurationMarshaller {

    private static final ObjectMapper OBJECT_MAPPER = new ObjectMapper();

    private ConfigurationMarshaller() {
        // Prevent outside initialization
    }

    public static void save(String fileName, Object obj) throws IOException {
        String json = OBJECT_MAPPER.writerWithDefaultPrettyPrinter().writeValueAsString(obj);

        try (FileWriter writer = new FileWriter(fileName, false)) {
            writer.write(json);
        }
    }

    public static String get(Object obj) throws JsonProcessingException {
        return OBJECT_MAPPER.writerWithDefaultPrettyPrinter().writeValueAsString(obj);
    }

    public static <T> T load(String fileName, Class<T> clazz) throws IOException {
        String json = Files.readString(Path.of(fileName));
        return OBJECT_MAPPER.readValue(json, clazz);
    }
}
