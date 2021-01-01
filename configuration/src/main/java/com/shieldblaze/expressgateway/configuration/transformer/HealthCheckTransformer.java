/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration.transformer;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfigurationBuilder;
import io.netty.util.internal.SystemPropertyUtil;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;

public class HealthCheckTransformer {

    private static final File FILE = new File(SystemPropertyUtil.get("egw.config.dir", "../bin/conf.d") + "/" + "HealthCheck.json");

    private HealthCheckTransformer() {
        // Prevent outside initialization
    }

    public static boolean write(HealthCheckConfiguration configuration) throws IOException {
        String jsonString = GSON.INSTANCE.toJson(configuration);

        try (FileWriter fileWriter = new FileWriter(FILE)) {
            fileWriter.write(jsonString);
        }

        return true;
    }

    public static String getFileData() throws IOException {
        return Files.readString(FILE.toPath());
    }

    public static HealthCheckConfiguration readFile() throws IOException {
        return readDirectly(getFileData());
    }

    public static HealthCheckConfiguration readDirectly(String data) {
        JsonObject json = JsonParser.parseString(data).getAsJsonObject();

        return HealthCheckConfigurationBuilder.newBuilder()
                .withWorkers(json.get("workers").getAsInt())
                .withTimeInterval(json.get("timeInterval").getAsInt())
                .build();
    }
}
