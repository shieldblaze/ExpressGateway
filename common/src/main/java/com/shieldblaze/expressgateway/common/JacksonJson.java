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
package com.shieldblaze.expressgateway.common;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;

/**
 * This class contains methods to read and write Json.
 */
public final class JacksonJson {

    public static final ObjectMapper OBJECT_MAPPER = new ObjectMapper();

    /**
     * Convert {@link Object} into Json {@link String} with pretty-printing
     *
     * @param obj {@link Object} to convert
     * @return Converted {@link String}
     * @throws JsonProcessingException In case of an error during conversion
     */
    public static String get(Object obj) throws JsonProcessingException {
        return OBJECT_MAPPER.writerWithDefaultPrettyPrinter().writeValueAsString(obj);
    }

    /**
     * Create a class from Json {@link String}
     *
     * @param json  Json {@link String}
     * @param clazz Class to create
     * @param <T>   Class type
     * @return Initialized class
     * @throws JsonProcessingException In case of an error during creation
     */
    public static <T> T read(String json, Class<T> clazz) throws JsonProcessingException {
        return OBJECT_MAPPER.readValue(json, clazz);
    }

    private JacksonJson() {
        // Prevent outside initialization
    }
}
