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
package com.shieldblaze.expressgateway.controlinterface.configuration;

import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfigurationBuilder;
import io.grpc.stub.StreamObserver;

public final class BufferService extends BufferServiceGrpc.BufferServiceImplBase {

    @Override
    public void buffer(Configuration.Buffer request, StreamObserver<Configuration.ConfigurationResponse> responseObserver) {
        Configuration.ConfigurationResponse response;

        try {
            BufferConfiguration bufferConfiguration = BufferConfigurationBuilder.newBuilder()
                    .withPreferDirect(request.getPreferDirect())
                    .withDirectMemoryCacheAlignment(request.getDirectMemoryCacheAlignment())
                    .withUseCacheForAllThreads(request.getUseCacheForAllThreads())
                    .withNormalCacheSize(request.getNormalCacheSize())
                    .withSmallCacheSize(request.getSmallCacheSize())
                    .withMaxOrder(request.getMaxOrder())
                    .withPageSize(request.getPageSize())
                    .withDirectArena(request.getDirectArena())
                    .withHeapArena(request.getHeapArena())
                    .build();

            bufferConfiguration.saveTo(request.getProfileName());

            response = Configuration.ConfigurationResponse.newBuilder()
                    .setResponseCode(1)
                    .setResponseText("Success")
                    .build();
        } catch (Exception ex) {
            response = Configuration.ConfigurationResponse.newBuilder()
                    .setResponseCode(-1)
                    .setResponseText("Error: " + ex.getLocalizedMessage())
                    .build();
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }

    @Override
    public void get(Configuration.GetBuffer request, StreamObserver<Configuration.Buffer> responseObserver) {
        try {
            BufferConfiguration bufferConfiguration = BufferConfiguration.loadFrom(request.getProfileName());

            Configuration.Buffer buffer = Configuration.Buffer.newBuilder()
                    .setPreferDirect(bufferConfiguration.preferDirect())
                    .setHeapArena(bufferConfiguration.heapArena())
                    .setDirectArena(bufferConfiguration.directArena())
                    .setPageSize(bufferConfiguration.pageSize())
                    .setMaxOrder(bufferConfiguration.maxOrder())
                    .setSmallCacheSize(bufferConfiguration.smallCacheSize())
                    .setNormalCacheSize(bufferConfiguration.normalCacheSize())
                    .setUseCacheForAllThreads(bufferConfiguration.useCacheForAllThreads())
                    .setDirectMemoryCacheAlignment(bufferConfiguration.directMemoryCacheAlignment())
                    .setProfileName(request.getProfileName())
                    .build();

            responseObserver.onNext(buffer);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(ex);
            responseObserver.onCompleted();
        }
    }
}
