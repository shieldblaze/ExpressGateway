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
package com.shieldblaze.expressgateway.file.config;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.util.internal.SystemPropertyUtil;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;

public class Transport {

    private static final String DEFAULT = "bin/conf.d/default/Transport.json";

    public static void main(String[] args) throws IOException {

        TransportConfiguration transportConfiguration = TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withTCPFastOpenMaximumPendingRequests(2147483647)
                .withBackendConnectTimeout(10000 * 5)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{65535})
                .withSocketReceiveBufferSize(2147483647)
                .withSocketSendBufferSize(2147483647)
                .withTCPConnectionBacklog(2147483647)
                .withConnectionIdleTimeout(1800000)
                .build();


//        Gson gson = new GsonBuilder().disableHtmlEscaping().setPrettyPrinting().create();
//        System.out.println(gson.toJson(transportConfiguration));

        loadConfiguration();
    }

    private static void loadConfiguration() throws IOException {
        String path = SystemPropertyUtil.get("config.dir", DEFAULT);
        JsonObject json = JsonParser.parseString(Files.readString(new File(path).toPath())).getAsJsonObject();

        List<Integer> sizesOf = new ArrayList<>();
        JsonArray recvBuffSizes = json.get("receiveBufferSizes").getAsJsonArray();
        for (JsonElement jsonElement : recvBuffSizes) {
            sizesOf.add(jsonElement.getAsInt());
        }
        int[] sizes = new int[sizesOf.size()];
        // ArrayList to Array Conversion
        for (int i =0; i < sizesOf.size(); i++) {
            sizes[i] = sizesOf.get(i);
        }

        TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.valueOf(json.get("transportType").getAsString()))
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.valueOf(json.get("receiveBufferAllocationType").getAsString()))
                .withReceiveBufferSizes(sizes)
                .withTCPConnectionBacklog(Integer.parseInt(json.get("tcpConnectionBacklog").getAsString()))
                .withSocketSendBufferSize(Integer.parseInt(json.get("socketSendBufferSize").getAsString()))
                .withSocketReceiveBufferSize(Integer.parseInt(json.get("socketReceiveBufferSize").getAsString()))
                .withTCPFastOpenMaximumPendingRequests(Integer.parseInt(json.get("tcpFastOpenMaximumPendingRequests").getAsString()))
                .withConnectionIdleTimeout(Integer.parseInt(json.get("connectionIdleTimeout").getAsString()))
                .withBackendConnectTimeout(Integer.parseInt(json.get("backendConnectTimeout").getAsString()))
                .build();
    }
}
