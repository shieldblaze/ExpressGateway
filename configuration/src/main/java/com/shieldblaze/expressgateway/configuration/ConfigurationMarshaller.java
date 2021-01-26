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

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.dataformat.yaml.YAMLFactory;
import com.fasterxml.jackson.dataformat.yaml.YAMLGenerator;
import com.shieldblaze.expressgateway.common.GSON;
import com.shieldblaze.expressgateway.common.utils.Profile;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.io.StringReader;
import java.lang.reflect.Type;
import java.nio.file.Files;

/**
 * {@linkplain ConfigurationMarshaller} handles Marshalling of Configuration class data
 * to/from JSON over disk (file-based).
 */
public class ConfigurationMarshaller {

    private static final ObjectMapper OBJECT_MAPPER = new ObjectMapper(new YAMLFactory().disable(YAMLGenerator.Feature.WRITE_DOC_START_MARKER));

    protected static <T> T loadFrom(Class<?> clazz, String filename) throws IOException {
        String location = Profile.ensure() + filename;
        return (T) OBJECT_MAPPER.readValue(Files.readString(new File(location).toPath()), clazz);
    }

    protected static void saveTo(Object obj, String filename) throws IOException {
        String location = Profile.ensure() + filename;
        OBJECT_MAPPER.writeValue(new FileWriter(location), obj);
    }
}
