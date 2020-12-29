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
import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfigurationBuilder;
import io.netty.util.internal.SystemPropertyUtil;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;

public class PooledByteBufAllocatorTransformer {

    private PooledByteBufAllocatorTransformer() {
        // Prevent outside initialization
    }

    private static final File FILE = new File(SystemPropertyUtil.get("egw.config.dir", "../bin/conf.d") + "/" + "PooledByteBufAllocator.json");

    public static boolean write(PooledByteBufAllocatorConfiguration configuration) throws IOException {
        String jsonString = GSON.INSTANCE.toJson(configuration);

        try (FileWriter fileWriter = new FileWriter(SystemPropertyUtil.get("egw.config.dir", "../bin/conf.d") + "/" + "PooledByteBufAllocator.json")) {
            fileWriter.write(jsonString);
        }

        return true;
    }

    public static String getFileData() throws IOException {
        return Files.readString(FILE.toPath());
    }

    public static PooledByteBufAllocatorConfiguration readFile() throws IOException {
        return readDirectly(getFileData());
    }

    public static PooledByteBufAllocatorConfiguration readDirectly(String data) {
        JsonObject json = JsonParser.parseString(data).getAsJsonObject();

        return PooledByteBufAllocatorConfigurationBuilder.newBuilder()
                .withPreferDirect(json.get("preferDirect").getAsBoolean())
                .withHeapArena(json.get("heapArena").getAsInt())
                .withDirectArena(json.get("directArena").getAsInt())
                .withPageSize(json.get("pageSize").getAsInt())
                .withMaxOrder(json.get("maxOrder").getAsInt())
                .withSmallCacheSize(json.get("smallCacheSize").getAsInt())
                .withNormalCacheSize(json.get("normalCacheSize").getAsInt())
                .withUseCacheForAllThreads(json.get("useCacheForAllThreads").getAsBoolean())
                .withDirectMemoryCacheAlignment(json.get("directMemoryCacheAlignment").getAsInt())
                .build();
    }
}
