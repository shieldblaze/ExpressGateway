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
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfigurationBuilder;
import io.netty.util.internal.SystemPropertyUtil;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;

public final class EventLoopTransformer {

    private static final File FILE = new File(SystemPropertyUtil.get("egw.config.dir", "../bin/conf.d") + "/" + "EventLoop.json");

    private EventLoopTransformer() {
        // Prevent outside initialization
    }

    public static void write(EventLoopConfiguration configuration) throws IOException {
        String jsonString = GSON.INSTANCE.toJson(configuration);

        try (FileWriter fileWriter = new FileWriter(FILE)) {
            fileWriter.write(jsonString);
        }
    }

    public static String getFileData() throws IOException {
        return Files.readString(FILE.toPath());
    }

    public static EventLoopConfiguration readFile() throws IOException {
        return readDirectly(getFileData());
    }

    public static EventLoopConfiguration readDirectly(String data) {
        JsonObject json = JsonParser.parseString(data).getAsJsonObject();

        return EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(json.get("parentWorkers").getAsInt())
                .withChildWorkers(json.get("childWorkers").getAsInt())
                .build();
    }
}
