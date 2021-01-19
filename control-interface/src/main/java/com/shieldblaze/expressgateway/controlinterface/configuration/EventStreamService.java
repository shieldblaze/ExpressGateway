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

import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfigurationBuilder;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;

public final class EventStreamService extends EventStreamServiceGrpc.EventStreamServiceImplBase {

    @Override
    public void eventstream(Configuration.EventStream request, StreamObserver<Configuration.ConfigurationResponse> responseObserver) {
        Configuration.ConfigurationResponse response;

        try {
            EventStreamConfiguration eventStreamConfiguration = EventStreamConfigurationBuilder.newBuilder()
                    .withWorkers(request.getWorkers())
                    .build();

            eventStreamConfiguration.saveTo(request.getProfileName());

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
    public void get(Configuration.GetEventStream request, StreamObserver<Configuration.EventStream> responseObserver) {
        try {
            EventStreamConfiguration eventStreamConfiguration = EventStreamConfiguration.loadFrom(request.getProfileName());
            Configuration.EventStream eventStream = Configuration.EventStream.newBuilder()
                    .setWorkers(eventStreamConfiguration.workers())
                    .setProfileName(request.getProfileName())
                    .build();

            responseObserver.onNext(eventStream);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
        }
    }
}
