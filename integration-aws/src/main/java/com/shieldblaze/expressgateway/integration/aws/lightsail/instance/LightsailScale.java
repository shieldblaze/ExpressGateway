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
package com.shieldblaze.expressgateway.integration.aws.lightsail.instance;

import com.shieldblaze.expressgateway.integration.server.ScaleIn;
import com.shieldblaze.expressgateway.integration.server.ScaleOut;
import com.shieldblaze.expressgateway.integration.aws.lightsail.events.LightsailScaleInEvent;
import com.shieldblaze.expressgateway.integration.aws.lightsail.events.LightsailScaleOutEvent;
import software.amazon.awssdk.services.lightsail.LightsailClient;
import software.amazon.awssdk.services.lightsail.model.CreateInstancesRequest;
import software.amazon.awssdk.services.lightsail.model.CreateInstancesResponse;
import software.amazon.awssdk.services.lightsail.model.DeleteInstanceRequest;
import software.amazon.awssdk.services.lightsail.model.DeleteInstanceResponse;
import software.amazon.awssdk.services.lightsail.model.IpAddressType;
import software.amazon.awssdk.services.lightsail.model.OperationStatus;
import software.amazon.awssdk.services.lightsail.model.Tag;

import java.util.Objects;

public final class LightsailScale implements ScaleIn<String, LightsailScaleInEvent>, ScaleOut<ScaleOutRequest, LightsailScaleOutEvent> {

    private final LightsailClient lightsailClient;

    public LightsailScale(LightsailClient lightsailClient) {
        this.lightsailClient = Objects.requireNonNull(lightsailClient, "LightsailClient");
    }

    @Override
    public LightsailScaleInEvent scaleIn(String instanceName) {
        LightsailScaleInEvent event = new LightsailScaleInEvent();

        try {
            DeleteInstanceResponse deleteInstanceResponse = lightsailClient.deleteInstance(DeleteInstanceRequest.builder()
                    .instanceName(instanceName)
                    .forceDeleteAddOns(true)
                    .build());

            if (!deleteInstanceResponse.hasOperations()) {
                throw new IllegalArgumentException("DeleteInstanceResponse does not have any Operations");
            }

            OperationStatus status = deleteInstanceResponse.operations().get(0).status();
            if (status == OperationStatus.SUCCEEDED) {
                event.trySuccess(deleteInstanceResponse);
            } else {
                event.tryFailure(new IllegalArgumentException("Instance state: " + status));
            }
        } catch (Exception ex) {
            event.tryFailure(ex);
        }

        return event;
    }

    @Override
    public LightsailScaleOutEvent scaleOut(ScaleOutRequest scaleOutRequest) {
        LightsailScaleOutEvent event = new LightsailScaleOutEvent();

        try {
            CreateInstancesResponse createInstancesResponse = lightsailClient.createInstances(CreateInstancesRequest.builder()
                    .instanceNames(scaleOutRequest.instanceName())
                    .availabilityZone(scaleOutRequest.availabilityZone())
                    .bundleId(scaleOutRequest.bundle().bundleName())
                    .blueprintId("debian_10")
                    .tags(Tag.builder().key("ExpressGateway").value(scaleOutRequest.autoscaled() ? "Autoscaled" : "Master").build())
                    .ipAddressType(IpAddressType.DUALSTACK)
                    .build());

            if (!createInstancesResponse.hasOperations()) {
                throw new IllegalArgumentException("CreateInstancesResponse does not have any Operations");
            }

            OperationStatus status = createInstancesResponse.operations().get(0).status();
            if (status == OperationStatus.SUCCEEDED) {
                event.trySuccess(createInstancesResponse);
            } else {
                event.tryFailure(new IllegalArgumentException("Instance state: " + status));
            }
        } catch (Exception ex) {
            event.tryFailure(ex);
        }

        return event;
    }
}
