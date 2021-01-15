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

import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfigurationBuilder;
import io.grpc.stub.StreamObserver;

public final class HealthCheckService extends HealthCheckServiceGrpc.HealthCheckServiceImplBase {

    @Override
    public void healthcheck(Configuration.HealthCheck request, StreamObserver<Configuration.ConfigurationResponse> responseObserver) {
        Configuration.ConfigurationResponse response;

        try {
            HealthCheckConfiguration healthCheckConfiguration = HealthCheckConfigurationBuilder.newBuilder()
                    .withWorkers(request.getWorkers())
                    .withTimeInterval(request.getTimeInterval())
                    .build();

            healthCheckConfiguration.saveTo(request.getProfileName());

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
    public void get(Configuration.GetHealthCheck request, StreamObserver<Configuration.HealthCheck> responseObserver) {
        try {
            HealthCheckConfiguration healthCheckConfiguration = HealthCheckConfiguration.loadFrom(request.getProfileName());

            Configuration.HealthCheck healthCheck = Configuration.HealthCheck.newBuilder()
                    .setTimeInterval(healthCheckConfiguration.timeInterval())
                    .setWorkers(healthCheckConfiguration.workers())
                    .setProfileName(request.getProfileName())
                    .build();

            responseObserver.onNext(healthCheck);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(ex);
            responseObserver.onCompleted();
        }
    }
}
