// Code generated by go-swagger; DO NOT EDIT.

package models

// This file was generated by the swagger tool.
// Editing this file might prove futile when you re-run the swagger generate command

import (
	"context"
	"encoding/json"

	"github.com/go-openapi/errors"
	"github.com/go-openapi/strfmt"
	"github.com/go-openapi/swag"
	"github.com/go-openapi/validate"
)

// InstanceInfo Describes MicroVM instance information.
//
// swagger:model InstanceInfo
type InstanceInfo struct {

	// Application name.
	// Required: true
	AppName *string `json:"app_name"`

	// MicroVM / instance ID.
	// Required: true
	ID *string `json:"id"`

	// The current detailed state (Not started, Running, Paused) of the Firecracker instance. This value is read-only for the control-plane.
	// Required: true
	// Enum: [Not started Running Paused]
	State *string `json:"state"`

	// MicroVM hypervisor build version.
	// Required: true
	VmmVersion *string `json:"vmm_version"`
}

// Validate validates this instance info
func (m *InstanceInfo) Validate(formats strfmt.Registry) error {
	var res []error

	if err := m.validateAppName(formats); err != nil {
		res = append(res, err)
	}

	if err := m.validateID(formats); err != nil {
		res = append(res, err)
	}

	if err := m.validateState(formats); err != nil {
		res = append(res, err)
	}

	if err := m.validateVmmVersion(formats); err != nil {
		res = append(res, err)
	}

	if len(res) > 0 {
		return errors.CompositeValidationError(res...)
	}
	return nil
}

func (m *InstanceInfo) validateAppName(formats strfmt.Registry) error {

	if err := validate.Required("app_name", "body", m.AppName); err != nil {
		return err
	}

	return nil
}

func (m *InstanceInfo) validateID(formats strfmt.Registry) error {

	if err := validate.Required("id", "body", m.ID); err != nil {
		return err
	}

	return nil
}

var instanceInfoTypeStatePropEnum []interface{}

func init() {
	var res []string
	if err := json.Unmarshal([]byte(`["Not started","Running","Paused"]`), &res); err != nil {
		panic(err)
	}
	for _, v := range res {
		instanceInfoTypeStatePropEnum = append(instanceInfoTypeStatePropEnum, v)
	}
}

const (

	// InstanceInfoStateNotStarted captures enum value "Not started"
	InstanceInfoStateNotStarted string = "Not started"

	// InstanceInfoStateRunning captures enum value "Running"
	InstanceInfoStateRunning string = "Running"

	// InstanceInfoStatePaused captures enum value "Paused"
	InstanceInfoStatePaused string = "Paused"
)

// prop value enum
func (m *InstanceInfo) validateStateEnum(path, location string, value string) error {
	if err := validate.EnumCase(path, location, value, instanceInfoTypeStatePropEnum, true); err != nil {
		return err
	}
	return nil
}

func (m *InstanceInfo) validateState(formats strfmt.Registry) error {

	if err := validate.Required("state", "body", m.State); err != nil {
		return err
	}

	// value enum
	if err := m.validateStateEnum("state", "body", *m.State); err != nil {
		return err
	}

	return nil
}

func (m *InstanceInfo) validateVmmVersion(formats strfmt.Registry) error {

	if err := validate.Required("vmm_version", "body", m.VmmVersion); err != nil {
		return err
	}

	return nil
}

// ContextValidate validates this instance info based on context it is used
func (m *InstanceInfo) ContextValidate(ctx context.Context, formats strfmt.Registry) error {
	return nil
}

// MarshalBinary interface implementation
func (m *InstanceInfo) MarshalBinary() ([]byte, error) {
	if m == nil {
		return nil, nil
	}
	return swag.WriteJSON(m)
}

// UnmarshalBinary interface implementation
func (m *InstanceInfo) UnmarshalBinary(b []byte) error {
	var res InstanceInfo
	if err := swag.ReadJSON(b, &res); err != nil {
		return err
	}
	*m = res
	return nil
}
