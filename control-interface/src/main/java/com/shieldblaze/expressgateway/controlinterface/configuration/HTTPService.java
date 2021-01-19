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

import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfigurationBuilder;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;

public final class HTTPService extends HTTPServiceGrpc.HTTPServiceImplBase {

    @Override
    public void http(Configuration.HTTP request, StreamObserver<Configuration.ConfigurationResponse> responseObserver) {
        Configuration.ConfigurationResponse response;

        try {
            HTTPConfiguration httpConfiguration = HTTPConfigurationBuilder.newBuilder()
                    .withMaxInitialLineLength(request.getMaxInitialLineLength())
                    .withMaxHeaderSize(request.getMaxHeaderSize())
                    .withMaxContentLength(request.getMaxContentLength())
                    .withMaxChunkSize(request.getMaxChunkSize())
                    .withH2MaxHeaderTableSize(request.getH2MaxHeaderTableSize())
                    .withH2MaxHeaderListSize(request.getH2MaxHeaderListSize())
                    .withH2MaxFrameSize(request.getH2MaxFrameSize())
                    .withH2MaxConcurrentStreams(request.getH2MaxConcurrentStreams())
                    .withH2InitialWindowSize(request.getH2InitialWindowSize())
                    .withDeflateCompressionLevel(request.getDeflateCompressionLevel())
                    .withCompressionThreshold(request.getCompressionThreshold())
                    .withBrotliCompressionLevel(request.getBrotliCompressionLevel())
                    .build();

            httpConfiguration.saveTo(request.getProfileName());

            response = Configuration.ConfigurationResponse.newBuilder()
                    .setSuccess(true)
                    .setResponseText("Success")
                    .build();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
            return;
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }

    @Override
    public void get(Configuration.GetHTTP request, StreamObserver<Configuration.HTTP> responseObserver) {
        try {
            HTTPConfiguration httpConfiguration = HTTPConfiguration.loadFrom(request.getProfileName());

            Configuration.HTTP http = Configuration.HTTP.newBuilder()
                    .setMaxContentLength(httpConfiguration.maxContentLength())
                    .setH2InitialWindowSize(httpConfiguration.h2InitialWindowSize())
                    .setH2MaxConcurrentStreams(httpConfiguration.h2MaxConcurrentStreams())
                    .setH2MaxHeaderListSize(httpConfiguration.h2MaxHeaderListSize())
                    .setH2MaxHeaderTableSize(httpConfiguration.h2MaxHeaderTableSize())
                    .setH2MaxFrameSize(httpConfiguration.h2MaxFrameSize())
                    .setMaxInitialLineLength(httpConfiguration.maxInitialLineLength())
                    .setMaxHeaderSize(httpConfiguration.maxHeaderSize())
                    .setMaxChunkSize(httpConfiguration.maxChunkSize())
                    .setCompressionThreshold(httpConfiguration.compressionThreshold())
                    .setDeflateCompressionLevel(httpConfiguration.deflateCompressionLevel())
                    .setBrotliCompressionLevel(httpConfiguration.brotliCompressionLevel())
                    .setProfileName(request.getProfileName())
                    .build();

            responseObserver.onNext(http);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
        }
    }
}
