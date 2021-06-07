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
package com.shieldblaze.expressgateway.integration.aws.lightsail;

import com.shieldblaze.expressgateway.integration.ScaleIn;
import com.shieldblaze.expressgateway.integration.aws.lightsail.events.LightsailScaleInEvent;
import software.amazon.awssdk.services.lightsail.LightsailClient;
import software.amazon.awssdk.services.lightsail.model.DeleteInstanceRequest;
import software.amazon.awssdk.services.lightsail.model.DeleteInstanceResponse;
import software.amazon.awssdk.services.lightsail.model.OperationStatus;

public final class LightsailScaleIn implements ScaleIn<LightsailScaleInEvent> {
    private final LightsailClient lightsailClient;
    private final String instanceName;

    public LightsailScaleIn(LightsailClient lightsailClient, String instanceName) {
        this.lightsailClient = lightsailClient;
        this.instanceName = instanceName;
    }

    @Override
    public LightsailScaleInEvent scaleIn() {
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
}
