from datetime import date, datetime  # noqa: F401

from typing import List, Dict  # noqa: F401

from gscoordinator.flex.models.base_model import Model
from gscoordinator.flex import util


class PrimitiveType(Model):
    """NOTE: This class is auto generated by OpenAPI Generator (https://openapi-generator.tech).

    Do not edit the class manually.
    """

    def __init__(self, primitive_type=None):  # noqa: E501
        """PrimitiveType - a model defined in OpenAPI

        :param primitive_type: The primitive_type of this PrimitiveType.  # noqa: E501
        :type primitive_type: str
        """
        self.openapi_types = {
            'primitive_type': str
        }

        self.attribute_map = {
            'primitive_type': 'primitive_type'
        }

        self._primitive_type = primitive_type

    @classmethod
    def from_dict(cls, dikt) -> 'PrimitiveType':
        """Returns the dict as a model

        :param dikt: A dict.
        :type: dict
        :return: The PrimitiveType of this PrimitiveType.  # noqa: E501
        :rtype: PrimitiveType
        """
        return util.deserialize_model(dikt, cls)

    @property
    def primitive_type(self) -> str:
        """Gets the primitive_type of this PrimitiveType.


        :return: The primitive_type of this PrimitiveType.
        :rtype: str
        """
        return self._primitive_type

    @primitive_type.setter
    def primitive_type(self, primitive_type: str):
        """Sets the primitive_type of this PrimitiveType.


        :param primitive_type: The primitive_type of this PrimitiveType.
        :type primitive_type: str
        """
        allowed_values = ["DT_SIGNED_INT32", "DT_UNSIGNED_INT32", "DT_SIGNED_INT64", "DT_UNSIGNED_INT64", "DT_BOOL", "DT_FLOAT", "DT_DOUBLE"]  # noqa: E501
        if primitive_type not in allowed_values:
            raise ValueError(
                "Invalid value for `primitive_type` ({0}), must be one of {1}"
                .format(primitive_type, allowed_values)
            )

        self._primitive_type = primitive_type