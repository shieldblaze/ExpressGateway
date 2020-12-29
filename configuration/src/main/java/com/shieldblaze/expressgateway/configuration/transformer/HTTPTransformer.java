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
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfigurationBuilder;
import io.netty.util.internal.SystemPropertyUtil;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;

public class HTTPTransformer {

    private HTTPTransformer() {
        // Prevent outside initialization
    }

    private static final File FILE = new File(SystemPropertyUtil.get("egw.config.dir", "../bin/conf.d") + "/" + "HTTP.json");

    public static boolean write(HTTPConfiguration configuration) throws IOException {
        String jsonString = GSON.INSTANCE.toJson(configuration);

        try (FileWriter fileWriter = new FileWriter(FILE)) {
            fileWriter.write(jsonString);
        }

        return true;
    }

    public static String getFileData() throws IOException {
        return Files.readString(FILE.toPath());
    }

    public static HTTPConfiguration readFile() throws IOException {
        return readDirectly(getFileData());
    }

    public static HTTPConfiguration readDirectly(String data) {
        JsonObject json = JsonParser.parseString(data).getAsJsonObject();

        return HTTPConfigurationBuilder.newBuilder()
                .withBrotliCompressionLevel(json.get("brotliCompressionLevel").getAsInt())
                .withCompressionThreshold(json.get("compressionThreshold").getAsInt())
                .withDeflateCompressionLevel(json.get("deflateCompressionLevel").getAsInt())
                .withMaxChunkSize(json.get("maxChunkSize").getAsInt())
                .withMaxContentLength(json.get("maxContentLength").getAsInt())
                .withMaxHeaderSize(json.get("maxHeaderSize").getAsInt())
                .withMaxInitialLineLength(json.get("maxInitialLineLength").getAsInt())
                .withH2InitialWindowSize(json.get("h2InitialWindowSize").getAsInt())
                .withH2MaxConcurrentStreams(json.get("h2MaxConcurrentStreams").getAsInt())
                .withH2MaxHeaderSizeList(json.get("h2MaxHeaderSizeList").getAsInt())
                .withH2MaxFrameSize(json.get("h2MaxFrameSize").getAsInt())
                .withH2MaxHeaderTableSize(json.get("h2MaxHeaderTableSize").getAsInt())
                .build();
    }
}
