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
package com.shieldblaze.expressgateway.configuration;

import com.shieldblaze.expressgateway.common.GSON;
import io.netty.util.internal.SystemPropertyUtil;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;

public final class Transformer {

    public static Configuration read(Configuration configuration, String profile) throws IOException {
        String data = readJSON(configuration, profile);
        return GSON.INSTANCE.fromJson(data, configuration.getClass());
    }

    public static String readJSON(Configuration configuration, String profile) throws IOException {
        makeDirs(profile);
        File file = new File(SystemPropertyUtil.get("expressgateway.config", "../bin/conf.d") + "/" + profile + "/" + configuration.name() + ".json");
        return Files.readString(file.toPath());
    }

    public static void write(Configuration configuration, String profile) throws IOException {
        makeDirs(profile);
        File file = new File(SystemPropertyUtil.get("expressgateway.config", "../bin/conf.d") + "/" + profile + "/" + configuration.name() + ".json");

        String data = GSON.INSTANCE.toJson(configuration);

        try (FileWriter fileWriter = new FileWriter(file, false)) {
            fileWriter.write(data);
        }
    }

    private static void makeDirs(String profile) {
        File file = new File(SystemPropertyUtil.get("expressgateway.config", "../bin/conf.d") + "/" + profile);
        file.mkdirs();
    }
}
