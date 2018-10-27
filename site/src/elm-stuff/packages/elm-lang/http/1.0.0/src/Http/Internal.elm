module Http.Internal exposing
  ( Request(..)
  , RawRequest
  , Expect
  , Body(..)
  , Header(..)
  , map
  )


import Native.Http
import Time exposing (Time)



type Request a = Request (RawRequest a)


type alias RawRequest a =
    { method : String
    , headers : List Header
    , url : String
    , body : Body
    , expect : Expect a
    , timeout : Maybe Time
    , withCredentials : Bool
    }


type Expect a = Expect


type Body
  = EmptyBody
  | StringBody String String
  | FormDataBody



type Header = Header String String


map : (a -> b) -> RawRequest a -> RawRequest b
map func request =
  { request | expect = Native.Http.mapExpect func request.expect }
