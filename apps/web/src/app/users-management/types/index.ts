import { UseFormReturn } from "react-hook-form";

import { User } from "@/http/endpoints/auth/types";
import { UserFormData } from "../hooks/use-user-management";

export interface UserActionsDropdownProps {
  user: User;
  isCurrentUser: boolean;
  onEdit: (user: User) => void;
  onDelete: (user: User) => void;
  onToggleStatus: (user: User) => void;
}

export interface UserDeleteModalProps {
  isOpen: boolean;
  onClose: () => void;
  user: User | null;
  onConfirm: () => Promise<void>;
}

export interface UserFormModalProps {
  isOpen: boolean;
  onClose: () => void;
  modalMode: "create" | "edit";
  selectedUser: User | null;
  formMethods: UseFormReturn<UserFormData>;
  onSubmit: (data: UserFormData) => Promise<void>;
}

export interface UserManagementModalsProps {
  modals: {
    isOpen: boolean;
    onClose: () => void;
    modalMode: "create" | "edit";
    isDeleteModalOpen: boolean;
    onDeleteModalClose: () => void;
    isStatusModalOpen: boolean;
    onStatusModalClose: () => void;
  };
  selectedUser: User | null;
  deleteModalUser: User | null;
  statusModalUser: User | null;
  onSubmit: (data: UserFormData) => Promise<void>;
  onDelete: () => Promise<void>;
  onToggleStatus: () => Promise<void>;
  formMethods: UseFormReturn<UserFormData>;
}

export interface UserStatusModalProps {
  isOpen: boolean;
  onClose: () => void;
  user: User | null;
  onConfirm: () => Promise<void>;
}

export interface UsersHeaderProps {
  onCreateUser: () => void;
  onGenerateInvite: () => void;
}

export interface AuthUser {
  id: string;
  firstName: string;
  lastName: string;
  username: string;
  email: string;
  image: string | null;
}

export interface UsersTableProps {
  users: User[];
  currentUser: AuthUser | null;
  onEdit: (user: User) => void;
  onDelete: (user: User) => void;
  onToggleStatus: (user: User) => void;
}
