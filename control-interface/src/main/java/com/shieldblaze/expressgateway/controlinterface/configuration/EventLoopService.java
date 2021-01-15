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

import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfigurationBuilder;
import io.grpc.stub.StreamObserver;

public final class EventLoopService extends EventLoopServiceGrpc.EventLoopServiceImplBase {

    @Override
    public void eventLoop(Configuration.EventLoop request, StreamObserver<Configuration.ConfigurationResponse> responseObserver) {
        Configuration.ConfigurationResponse response;

        try {
            EventLoopConfiguration eventLoopConfiguration = EventLoopConfigurationBuilder.newBuilder()
                    .withParentWorkers(request.getParentWorkers())
                    .withChildWorkers(request.getChildWorkers())
                    .build();

            eventLoopConfiguration.saveTo(request.getProfileName());

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
    public void get(Configuration.GetEventLoop request, StreamObserver<Configuration.EventLoop> responseObserver) {
        try {
            EventLoopConfiguration eventLoopConfiguration = EventLoopConfiguration.loadFrom(request.getProfileName());

            Configuration.EventLoop eventLoop = Configuration.EventLoop.newBuilder()
                    .setParentWorkers(eventLoopConfiguration.parentWorkers())
                    .setChildWorkers(eventLoopConfiguration.childWorkers())
                    .setProfileName(request.getProfileName())
                    .build();

            responseObserver.onNext(eventLoop);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(ex);
            responseObserver.onCompleted();
        }
    }
}
