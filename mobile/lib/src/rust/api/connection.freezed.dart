// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'connection.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$ConnectionStatus {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ConnectionStatus);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ConnectionStatus()';
}


}

/// @nodoc
class $ConnectionStatusCopyWith<$Res>  {
$ConnectionStatusCopyWith(ConnectionStatus _, $Res Function(ConnectionStatus) __);
}


/// Adds pattern-matching-related methods to [ConnectionStatus].
extension ConnectionStatusPatterns on ConnectionStatus {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ConnectionStatus_Disconnected value)?  disconnected,TResult Function( ConnectionStatus_Connecting value)?  connecting,TResult Function( ConnectionStatus_Connected value)?  connected,TResult Function( ConnectionStatus_Pairing value)?  pairing,TResult Function( ConnectionStatus_Error value)?  error,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ConnectionStatus_Disconnected() when disconnected != null:
return disconnected(_that);case ConnectionStatus_Connecting() when connecting != null:
return connecting(_that);case ConnectionStatus_Connected() when connected != null:
return connected(_that);case ConnectionStatus_Pairing() when pairing != null:
return pairing(_that);case ConnectionStatus_Error() when error != null:
return error(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ConnectionStatus_Disconnected value)  disconnected,required TResult Function( ConnectionStatus_Connecting value)  connecting,required TResult Function( ConnectionStatus_Connected value)  connected,required TResult Function( ConnectionStatus_Pairing value)  pairing,required TResult Function( ConnectionStatus_Error value)  error,}){
final _that = this;
switch (_that) {
case ConnectionStatus_Disconnected():
return disconnected(_that);case ConnectionStatus_Connecting():
return connecting(_that);case ConnectionStatus_Connected():
return connected(_that);case ConnectionStatus_Pairing():
return pairing(_that);case ConnectionStatus_Error():
return error(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ConnectionStatus_Disconnected value)?  disconnected,TResult? Function( ConnectionStatus_Connecting value)?  connecting,TResult? Function( ConnectionStatus_Connected value)?  connected,TResult? Function( ConnectionStatus_Pairing value)?  pairing,TResult? Function( ConnectionStatus_Error value)?  error,}){
final _that = this;
switch (_that) {
case ConnectionStatus_Disconnected() when disconnected != null:
return disconnected(_that);case ConnectionStatus_Connecting() when connecting != null:
return connecting(_that);case ConnectionStatus_Connected() when connected != null:
return connected(_that);case ConnectionStatus_Pairing() when pairing != null:
return pairing(_that);case ConnectionStatus_Error() when error != null:
return error(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  disconnected,TResult Function()?  connecting,TResult Function()?  connected,TResult Function()?  pairing,TResult Function( String message)?  error,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ConnectionStatus_Disconnected() when disconnected != null:
return disconnected();case ConnectionStatus_Connecting() when connecting != null:
return connecting();case ConnectionStatus_Connected() when connected != null:
return connected();case ConnectionStatus_Pairing() when pairing != null:
return pairing();case ConnectionStatus_Error() when error != null:
return error(_that.message);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  disconnected,required TResult Function()  connecting,required TResult Function()  connected,required TResult Function()  pairing,required TResult Function( String message)  error,}) {final _that = this;
switch (_that) {
case ConnectionStatus_Disconnected():
return disconnected();case ConnectionStatus_Connecting():
return connecting();case ConnectionStatus_Connected():
return connected();case ConnectionStatus_Pairing():
return pairing();case ConnectionStatus_Error():
return error(_that.message);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  disconnected,TResult? Function()?  connecting,TResult? Function()?  connected,TResult? Function()?  pairing,TResult? Function( String message)?  error,}) {final _that = this;
switch (_that) {
case ConnectionStatus_Disconnected() when disconnected != null:
return disconnected();case ConnectionStatus_Connecting() when connecting != null:
return connecting();case ConnectionStatus_Connected() when connected != null:
return connected();case ConnectionStatus_Pairing() when pairing != null:
return pairing();case ConnectionStatus_Error() when error != null:
return error(_that.message);case _:
  return null;

}
}

}

/// @nodoc


class ConnectionStatus_Disconnected extends ConnectionStatus {
  const ConnectionStatus_Disconnected(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ConnectionStatus_Disconnected);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ConnectionStatus.disconnected()';
}


}




/// @nodoc


class ConnectionStatus_Connecting extends ConnectionStatus {
  const ConnectionStatus_Connecting(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ConnectionStatus_Connecting);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ConnectionStatus.connecting()';
}


}




/// @nodoc


class ConnectionStatus_Connected extends ConnectionStatus {
  const ConnectionStatus_Connected(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ConnectionStatus_Connected);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ConnectionStatus.connected()';
}


}




/// @nodoc


class ConnectionStatus_Pairing extends ConnectionStatus {
  const ConnectionStatus_Pairing(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ConnectionStatus_Pairing);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ConnectionStatus.pairing()';
}


}




/// @nodoc


class ConnectionStatus_Error extends ConnectionStatus {
  const ConnectionStatus_Error({required this.message}): super._();
  

 final  String message;

/// Create a copy of ConnectionStatus
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ConnectionStatus_ErrorCopyWith<ConnectionStatus_Error> get copyWith => _$ConnectionStatus_ErrorCopyWithImpl<ConnectionStatus_Error>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ConnectionStatus_Error&&(identical(other.message, message) || other.message == message));
}


@override
int get hashCode => Object.hash(runtimeType,message);

@override
String toString() {
  return 'ConnectionStatus.error(message: $message)';
}


}

/// @nodoc
abstract mixin class $ConnectionStatus_ErrorCopyWith<$Res> implements $ConnectionStatusCopyWith<$Res> {
  factory $ConnectionStatus_ErrorCopyWith(ConnectionStatus_Error value, $Res Function(ConnectionStatus_Error) _then) = _$ConnectionStatus_ErrorCopyWithImpl;
@useResult
$Res call({
 String message
});




}
/// @nodoc
class _$ConnectionStatus_ErrorCopyWithImpl<$Res>
    implements $ConnectionStatus_ErrorCopyWith<$Res> {
  _$ConnectionStatus_ErrorCopyWithImpl(this._self, this._then);

  final ConnectionStatus_Error _self;
  final $Res Function(ConnectionStatus_Error) _then;

/// Create a copy of ConnectionStatus
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? message = null,}) {
  return _then(ConnectionStatus_Error(
message: null == message ? _self.message : message // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

// dart format on
